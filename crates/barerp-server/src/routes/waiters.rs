//! `/api/waiters`: garçons (o APK lista na abertura da mesa). Tradução de `server/src/routes/waiters.ts`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use barerp_db::waiters::{self, WaiterUpdate};

use crate::error::ApiResult;
use crate::http::Body;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/:id", put(update).delete(remove))
}

#[derive(Deserialize)]
struct ListQuery {
    all: Option<String>,
}

async fn list(State(state): State<AppState>, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let list = waiters::list(&state.db, q.all.as_deref() == Some("true")).await?;
    Ok(Json(json!(list)))
}

async fn create(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = body.str("name").unwrap_or_default();
    let code = body.str("code");
    let rate = body.num("commissionRate");
    let waiter = waiters::create(&state.db, &name, code.as_deref(), rate).await?;
    Ok((StatusCode::CREATED, Json(json!(waiter))))
}

async fn update(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let u = WaiterUpdate {
        name: body.str("name"),
        // `code: ""` apaga o código; ausente mantém
        code: body.0.get("code").map(|v| v.as_str().map(str::to_string).unwrap_or_default()),
        commission_rate: body.num("commissionRate"),
        active: body.bool("active"),
    };
    Ok(Json(json!(waiters::update(&state.db, &id, u).await?)))
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let removal = waiters::remove(&state.db, &id).await?;
    if removal.deactivated {
        let waiter = waiters::get(&state.db, &id).await?;
        return Ok(Json(json!({ "success": true, "message": removal.message, "waiter": waiter })));
    }
    Ok(Json(json!({ "success": true, "message": removal.message })))
}
