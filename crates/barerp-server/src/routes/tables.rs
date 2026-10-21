//! `/api/tables`: mapa de mesas, cadastro e as ações do salão (abrir, fechar conta, transferir, juntar).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use barerp_core::ids;
use barerp_db::events::Event;
use barerp_db::orders::{self, OpenTable};
use barerp_db::DbTime;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::events::{broadcast, emit_all};
use crate::http::Body;
use crate::prisma::{self, str_field, OrderInclude, Sort};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/batch/section", put(batch_section))
        .route("/sections/rename", post(rename_section))
        .route("/:id", put(update).delete(remove))
        .route("/:id/open", post(open))
        .route("/:id/request-closing", post(request_closing))
        .route("/:id/reopen", post(reopen))
        .route("/:id/transfer", post(transfer))
        .route("/:id/merge", post(merge))
}

/// Todas as mesas com `orders` (comandas abertas, itens mais novos primeiro) e `activeOrder`.
async fn list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mut conn = state.db.pool().acquire().await?;
    let tables = prisma::select(&mut conn, "Table", r#"ORDER BY "number" ASC"#, &[]).await?;
    let open = prisma::select(&mut conn, "Order", r#"WHERE "status" = 'OPEN' ORDER BY rowid"#, &[]).await?;
    let open = prisma::orders_with(
        &mut conn,
        open,
        OrderInclude { table: false, items: Some(Sort::Desc), payments: Some(Sort::None) },
    )
    .await?;

    let list: Vec<Value> = tables
        .into_iter()
        .map(|mut table| {
            let id = str_field(&table, "id").unwrap_or_default().to_string();
            let orders: Vec<Value> = open
                .iter()
                .filter(|o| str_field(o, "tableId") == Some(id.as_str()))
                .cloned()
                .map(Value::Object)
                .collect();
            table.insert("activeOrder".into(), orders.first().cloned().unwrap_or(Value::Null));
            table.insert("orders".into(), Value::Array(orders));
            Value::Object(table)
        })
        .collect();
    Ok(Json(Value::Array(list)))
}

async fn table_json(state: &AppState, id: &str) -> ApiResult<Value> {
    let mut conn = state.db.pool().acquire().await?;
    prisma::select_one(&mut conn, "Table", id)
        .await?
        .map(Value::Object)
        .ok_or_else(|| ApiError::not_found("Mesa não encontrada"))
}

fn table_event(state: &AppState, table_id: &str, action: &str) {
    broadcast(state, "table:updated", json!({ "tableId": table_id, "action": action }));
}

async fn number_taken(conn: &mut sqlx::SqliteConnection, number: i64) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(r#"SELECT "id" FROM "Table" WHERE "number" = ?"#).bind(number).fetch_optional(conn).await
}

async fn create(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    let number = body.num("number").map(|n| n as i64).unwrap_or(0);
    let mut tx = state.db.begin_write().await?;
    if number_taken(&mut tx, number).await?.is_some() {
        return Err(ApiError::bad_request(format!("Mesa {number} já existe")));
    }
    let id = ids::cuid();
    let now = DbTime::now();
    sqlx::query(
        r#"INSERT INTO "Table" ("id", "number", "name", "capacity", "status", "section", "customerCount", "createdAt", "updatedAt")
           VALUES (?, ?, ?, ?, 'AVAILABLE', ?, 1, ?, ?)"#,
    )
    .bind(&id)
    .bind(number)
    .bind(body.str("name").unwrap_or_else(|| format!("Mesa {number}")))
    .bind(body.num("capacity").map(|c| c as i64).filter(|c| *c != 0).unwrap_or(4))
    .bind(body.str("section").unwrap_or_else(|| "Salão Principal".into()))
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    table_event(&state, &id, "created");
    Ok((StatusCode::CREATED, Json(table_json(&state, &id).await?)))
}

async fn update(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let mut tx = state.db.begin_write().await?;
    let current: Option<(i64, Option<String>, i64, String)> =
        sqlx::query_as(r#"SELECT "number", "name", "capacity", "section" FROM "Table" WHERE "id" = ?"#)
            .bind(&id)
            .fetch_optional(&mut **tx)
            .await?;
    let (number, name, capacity, section) = current.ok_or_else(|| ApiError::not_found("Mesa não encontrada"))?;

    let new_number = body.get("number").and_then(crate::http::js_number).map(|n| n as i64);
    if let Some(n) = new_number.filter(|n| *n != 0 && *n != number) {
        if number_taken(&mut tx, n).await?.is_some_and(|other| other != id) {
            return Err(ApiError::bad_request(format!("Já existe uma mesa cadastrada com o número {n}")));
        }
    }
    let name = match body.0.get("name") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Null) => None,
        _ => name,
    };
    sqlx::query(
        r#"UPDATE "Table" SET "number" = ?, "name" = ?, "capacity" = ?, "section" = ?, "updatedAt" = ? WHERE "id" = ?"#,
    )
    .bind(new_number.unwrap_or(number))
    .bind(name)
    .bind(body.num("capacity").map(|c| c as i64).unwrap_or(capacity))
    .bind(body.get("section").map(|s| s.as_str().map(String::from).unwrap_or_else(|| s.to_string()).trim().to_string()).unwrap_or(section))
    .bind(DbTime::now())
    .bind(&id)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    table_event(&state, &id, "edited");
    Ok(Json(table_json(&state, &id).await?))
}

async fn batch_section(State(state): State<AppState>, body: Body) -> ApiResult<Json<Value>> {
    let (Some(ids), Some(section)) = (body.array("tableIds"), body.str("section")) else {
        return Err(ApiError::bad_request("tableIds e section são obrigatórios"));
    };
    let ids: Vec<String> = ids.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    let mut tx = state.db.begin_write().await?;
    let now = DbTime::now();
    for id in &ids {
        sqlx::query(r#"UPDATE "Table" SET "section" = ?, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(section.trim())
            .bind(now)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    tx.commit().await?;
    broadcast(&state, "table:updated", json!({ "action": "batch_section_updated" }));
    Ok(Json(json!({ "success": true, "count": body.array("tableIds").map_or(0, Vec::len) })))
}

async fn rename_section(State(state): State<AppState>, body: Body) -> ApiResult<Json<Value>> {
    let (Some(old), Some(new)) = (body.str("oldSection"), body.str("newSection")) else {
        return Err(ApiError::bad_request("Local antigo e novo são obrigatórios"));
    };
    let mut tx = state.db.begin_write().await?;
    let count = sqlx::query(r#"UPDATE "Table" SET "section" = ?, "updatedAt" = ? WHERE "section" = ?"#)
        .bind(new.trim())
        .bind(DbTime::now())
        .bind(old.trim())
        .execute(&mut **tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    broadcast(
        &state,
        "table:updated",
        json!({ "action": "section_renamed", "oldSection": body.get("oldSection"), "newSection": body.get("newSection") }),
    );
    Ok(Json(json!({ "success": true, "count": count })))
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let mut tx = state.db.begin_write().await?;
    let table: Option<(i64, String)> = sqlx::query_as(r#"SELECT "number", "status" FROM "Table" WHERE "id" = ?"#)
        .bind(&id)
        .fetch_optional(&mut **tx)
        .await?;
    let (number, status) = table.ok_or_else(|| ApiError::not_found("Mesa não encontrada"))?;
    if status != "AVAILABLE" {
        return Err(ApiError::bad_request("Apenas mesas livres podem ser excluídas"));
    }
    sqlx::query(r#"DELETE FROM "Table" WHERE "id" = ?"#).bind(&id).execute(&mut **tx).await?;
    tx.commit().await?;
    table_event(&state, &id, "deleted");
    Ok(Json(json!({ "success": true, "message": format!("Mesa {number} excluída com sucesso") })))
}

async fn open(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let input = OpenTable {
        customer_name: body.str("customerName"),
        waiter_id: body.str("waiterId"),
        waiter_name: body.str("waiterName"),
        customer_count: body.num("customerCount").map(|n| n as i64).filter(|n| *n != 0).unwrap_or(1),
    };
    let (order_id, events) = orders::open_table(&state.db, &id, input).await?;
    emit_all(&state, &events).await;
    let mut conn = state.db.pool().acquire().await?;
    let order = prisma::select_one(&mut conn, "Order", &order_id).await?.map(Value::Object).unwrap_or(Value::Null);
    drop(conn);
    Ok(Json(json!({ "table": table_json(&state, &id).await?, "order": order })))
}

async fn status_action(state: &AppState, id: &str, events: Vec<Event>) -> ApiResult<Json<Value>> {
    emit_all(state, &events).await;
    Ok(Json(table_json(state, id).await?))
}

async fn request_closing(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let events = orders::request_closing(&state.db, &id).await?;
    status_action(&state, &id, events).await
}

async fn reopen(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let events = orders::reopen(&state.db, &id).await?;
    status_action(&state, &id, events).await
}

async fn transfer(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let target = body.str("targetTableId").unwrap_or_default();
    let (message, events) = orders::transfer_table(&state.db, &id, &target).await?;
    emit_all(&state, &events).await;
    Ok(Json(json!({ "success": true, "message": message })))
}

async fn merge(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let second = body.str("secondTableId").unwrap_or_default();
    let (message, events) = orders::merge_tables(&state.db, &id, &second).await?;
    emit_all(&state, &events).await;
    Ok(Json(json!({ "success": true, "message": message })))
}
