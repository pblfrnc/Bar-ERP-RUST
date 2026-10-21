//! `/api/cash`: abertura de turno, sangria, suprimento, fechamento cego e reabertura.
//! Tradução de `server/src/routes/cash.ts`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use barerp_db::cash::{self, CloseShiftInput, OpenShiftInput, TransactionInput};

use crate::error::{ApiError, ApiResult};
use crate::events::emit_all;
use crate::http::js_number;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/current", get(current))
        .route("/open", post(open))
        .route("/transaction", post(transaction))
        .route("/close", post(close))
        .route("/reopen-last", post(reopen_last))
        .route("/history", get(history))
}

async fn current(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let shift_state = cash::current_shift(&state.db).await?;
    if !shift_state.is_open {
        return Ok(Json(json!({ "isOpen": false, "shift": Value::Null })));
    }
    Ok(Json(json!({
        "isOpen": true,
        "shift": shift_state.shift,
        "summary": shift_state.summary,
    })))
}

async fn open(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let initial_balance = body.get("initialBalance").and_then(js_number).unwrap_or(0.0);
    let opened_by = body.get("openedBy").and_then(Value::as_str).map(String::from);
    let notes = body.get("notes").and_then(Value::as_str).map(String::from);

    let (shift, events) = cash::open_shift(
        &state.db,
        OpenShiftInput {
            initial_balance,
            opened_by,
            notes,
        },
    )
    .await?;

    emit_all(&state, &events).await;
    Ok((StatusCode::CREATED, Json(serde_json::to_value(shift).unwrap_or(Value::Null))))
}

async fn transaction(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let tx_type = body
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("Tipo, valor e motivo são obrigatórios"))?;
    let amount = body
        .get("amount")
        .and_then(js_number)
        .ok_or_else(|| ApiError::bad_request("Tipo, valor e motivo são obrigatórios"))?;
    let reason = body
        .get("reason")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("Tipo, valor e motivo são obrigatórios"))?;

    let (tx, events) = cash::add_transaction(
        &state.db,
        TransactionInput {
            tx_type: tx_type.to_string(),
            amount,
            reason: reason.to_string(),
        },
    )
    .await?;

    emit_all(&state, &events).await;
    Ok((StatusCode::CREATED, Json(serde_json::to_value(tx).unwrap_or(Value::Null))))
}

async fn close(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let final_cash_count = body.get("finalCashCount").and_then(js_number).unwrap_or(0.0);
    let closed_by = body.get("closedBy").and_then(Value::as_str).map(String::from);
    let notes = body.get("notes").and_then(Value::as_str).map(String::from);

    let (closed_shift, report, events) = cash::close_shift(
        &state.db,
        CloseShiftInput {
            final_cash_count,
            closed_by,
            notes,
        },
    )
    .await?;

    emit_all(&state, &events).await;

    Ok(Json(json!({
        "success": true,
        "shift": closed_shift,
        "report": report,
    })))
}

async fn reopen_last(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (reopened, events) = cash::reopen_last_shift(&state.db).await?;
    emit_all(&state, &events).await;

    Ok(Json(json!({
        "success": true,
        "shift": reopened,
        "message": "Turno de caixa reaberto com sucesso para correção!",
    })))
}

async fn history(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let shifts = cash::shift_history(&state.db, 30).await?;
    Ok(Json(serde_json::to_value(shifts).unwrap_or(Value::Null)))
}
