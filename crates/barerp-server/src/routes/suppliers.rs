//! `/api/suppliers`: fornecedores e distribuidoras. Tradução de `server/src/routes/suppliers.ts`
//! (o modo admin do APK usa; no PC a tela é da área Cardápio).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use barerp_db::suppliers::{self, Supplier, SupplierInput};

use crate::error::ApiResult;
use crate::http::Body;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/:id", get(detail).put(update).delete(remove))
}

/// Fornecedor no formato do Prisma, com `_count.products`.
fn supplier_json(s: &Supplier) -> Value {
    let mut v = json!(s);
    v["_count"] = json!({ "products": s.product_count });
    v
}

#[derive(Deserialize)]
struct ListQuery {
    search: Option<String>,
}

async fn list(State(state): State<AppState>, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let list = suppliers::list(&state.db, q.search.as_deref().map(str::trim).filter(|s| !s.is_empty())).await?;
    Ok(Json(Value::Array(list.iter().map(supplier_json).collect())))
}

async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let d = suppliers::detail(&state.db, &id).await?;
    let mut v = supplier_json(&d.supplier);
    v["products"] = json!(d.products);
    Ok(Json(v))
}

/// Campo do corpo: ausente = não mexe; `null` ou texto = valor (texto vazio apaga).
fn field(body: &Body, key: &str) -> Option<String> {
    body.0.get(key).map(|v| match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

fn input(body: &Body) -> SupplierInput {
    SupplierInput {
        name: field(body, "name"),
        trade_name: field(body, "tradeName"),
        document: field(body, "document"),
        ie: field(body, "ie"),
        phone: field(body, "phone"),
        email: field(body, "email"),
        city: field(body, "city"),
        state: field(body, "state"),
        address: field(body, "address"),
        contact_name: field(body, "contactName"),
        notes: field(body, "notes"),
    }
}

async fn create(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    let s = suppliers::create(&state.db, input(&body)).await?;
    Ok((StatusCode::CREATED, Json(supplier_json(&s))))
}

async fn update(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let s = suppliers::update(&state.db, &id, input(&body)).await?;
    Ok(Json(supplier_json(&s)))
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    suppliers::delete(&state.db, &id).await?;
    Ok(Json(json!({ "success": true, "message": "Fornecedor removido com sucesso" })))
}
