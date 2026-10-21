//! `/api/kds`: fila de produção do bar e da cozinha, avanço de status e atualizações em lote.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use barerp_db::kds::{self, KdsItem};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiResult;
use crate::events::emit_all;
use crate::http::Body;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_queue))
        .route("/items/:item_id/status", put(update_item_status))
        .route("/orders/:order_id/ready-all", post(ready_all))
        .route("/orders/:order_id/batch-status", post(batch_status))
}

#[derive(Deserialize)]
pub struct QueueQuery {
    pub station: Option<String>,
}

fn item_to_json(item: KdsItem) -> Value {
    let order_table = item.table_id.as_ref().map(|tid| {
        json!({
            "id": tid,
            "number": item.table_number,
            "name": item.table_name,
        })
    });

    json!({
        "id": item.id,
        "orderId": item.order_id,
        "productId": item.product_id,
        "productName": item.product_name,
        "quantity": item.quantity,
        "unitType": item.unit_type,
        "notes": item.notes,
        "modifiers": item.modifiers,
        "kdsStatus": item.kds_status,
        "kdsStation": item.kds_station,
        "addedAt": item.added_at,
        "orderNumber": item.order_number,
        "tableId": item.table_id,
        "tableNumber": item.table_number,
        "tableName": item.table_name,
        "waiterId": item.waiter_id,
        "waiterName": item.waiter_name,
        "customerName": item.customer_name,
        "product": {
            "id": item.product_id,
            "name": item.product_name,
        },
        "order": {
            "id": item.order_id,
            "orderNumber": item.order_number,
            "tableId": item.table_id,
            "waiterId": item.waiter_id,
            "waiterName": item.waiter_name,
            "customerName": item.customer_name,
            "table": order_table,
        }
    })
}

async fn list_queue(
    State(state): State<AppState>,
    Query(query): Query<QueueQuery>,
) -> ApiResult<Json<Value>> {
    let items = kds::queue(&state.db, query.station.as_deref()).await?;
    let json_items: Vec<Value> = items.into_iter().map(item_to_json).collect();
    Ok(Json(Value::Array(json_items)))
}

async fn update_item_status(
    State(state): State<AppState>,
    Path(item_id): Path<String>,
    body: Body,
) -> ApiResult<Json<Value>> {
    let status = body.str("status").unwrap_or_default();
    let (updated, events) = kds::set_item_status(&state.db, &item_id, &status).await?;
    emit_all(&state, &events).await;
    Ok(Json(item_to_json(updated)))
}

async fn ready_all(
    State(state): State<AppState>,
    Path(order_id): Path<String>,
    body: Body,
) -> ApiResult<Json<Value>> {
    let station = body.str("station");
    let events = kds::ready_all(&state.db, &order_id, station.as_deref()).await?;
    emit_all(&state, &events).await;
    Ok(Json(json!({
        "success": true,
        "message": "Todos os itens foram marcados como prontos"
    })))
}

async fn batch_status(
    State(state): State<AppState>,
    Path(order_id): Path<String>,
    body: Body,
) -> ApiResult<Json<Value>> {
    let status = body.str("status").unwrap_or_default();
    let station = body.str("station");
    let events = kds::batch_status(&state.db, &order_id, &status, station.as_deref()).await?;
    emit_all(&state, &events).await;
    Ok(Json(json!({
        "success": true,
        "message": format!("Itens atualizados para {status}")
    })))
}
