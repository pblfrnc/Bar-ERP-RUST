//! `/api/customers`: clientes do fiado (o APK lista e cadastra no fechamento da conta).
//! Tradução de `server/src/routes/customers.ts`; telefone, documento e observações abertos só na resposta.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use barerp_db::customers::{self, NewCustomer};

use crate::error::ApiResult;
use crate::http::Body;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list).post(create))
}

async fn list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(customers::list(&state.db).await?)))
}

async fn create(State(state): State<AppState>, body: Body) -> ApiResult<Json<Value>> {
    let input = NewCustomer {
        name: body.str("name").unwrap_or_default(),
        phone: body.str("phone"),
        document: body.str("document"),
        notes: body.str("notes"),
    };
    Ok(Json(json!(customers::create(&state.db, input).await?)))
}
