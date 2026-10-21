//! `/api/orders`: comanda, lançamento e cancelamento de itens, 10%, desconto, histórico e o recibo
//! impresso no caixa a pedido do celular.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use barerp_db::orders::{self, NewLine};
use chrono::{Local, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{oneshot, OnceCell};

use crate::error::{ApiError, ApiResult};
use crate::events::emit_all;
use crate::http::{js_number, Body};
use crate::prisma::{self, OrderInclude, Sort};
use crate::socket::{PrintRequest, PRINT_STATIONS_ROOM};
use crate::AppState;

/// Tempo que o caixa tem para confirmar a impressão do recibo.
const PRINT_TIMEOUT: Duration = Duration::from_secs(8);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/history", get(history))
        .route("/quick-sale", post(quick_sale))
        .route("/:id", get(detail))
        .route("/:id/items", post(add_items))
        .route("/:id/items/:item_id", delete(remove_item))
        .route("/:id/service-fee", put(service_fee))
        .route("/:id/discount", put(discount))
        .route("/:id/pay", post(pay))
        .route("/:id/print-receipt", post(print_receipt))
        .route("/:id/loss", post(loss))
}

async fn order_json(state: &AppState, id: &str, include: OrderInclude) -> ApiResult<Value> {
    let mut conn = state.db.pool().acquire().await?;
    prisma::find_order(&mut conn, id, include)
        .await?
        .map(Value::Object)
        .ok_or_else(|| ApiError::not_found("Comanda não encontrada"))
}

/// `include: { table: true, items: { include: { product: true }, orderBy: { addedAt: 'desc' } }, payments: { orderBy: { receivedAt: 'desc' } } }`
const DETAIL: OrderInclude = OrderInclude { table: true, items: Some(Sort::Desc), payments: Some(Sort::Desc) };

async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(order_json(&state, &id, DETAIL).await?))
}

fn parse_lines(items: &[Value]) -> Vec<NewLine> {
    items
        .iter()
        .filter_map(Value::as_object)
        .map(|item| NewLine {
            product_id: item.get("productId").and_then(Value::as_str).unwrap_or_default().to_string(),
            quantity: item.get("quantity").and_then(js_number).map(|q| q as i64).filter(|q| *q != 0).unwrap_or(1),
            notes: item.get("notes").and_then(Value::as_str).filter(|n| !n.is_empty()).map(String::from),
            unit_type: item.get("unitType").and_then(Value::as_str).map(String::from),
            modifier_option_ids: item
                .get("modifierOptionIds")
                .and_then(Value::as_array)
                .map(|ids| ids.iter().filter_map(|v| v.as_str().map(String::from)).collect()),
        })
        .collect()
}

async fn add_items(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Body,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let lines = body.array("items").map(|items| parse_lines(items)).unwrap_or_default();
    if lines.is_empty() {
        return Err(ApiError::bad_request("Nenhum item informado"));
    }
    let (created, events) = orders::add_items(&state.db, &id, &lines, Local::now().naive_local()).await?;
    emit_all(&state, &events).await;

    let order = order_json(&state, &id, OrderInclude::FULL).await?;
    let mut conn = state.db.pool().acquire().await?;
    let mut added = prisma::select_in(&mut conn, "OrderItem", "id", &created, r#"ORDER BY "addedAt" ASC, rowid ASC"#).await?;
    prisma::attach_products(&mut conn, &mut added).await?;
    Ok((StatusCode::CREATED, Json(json!({ "order": order, "addedItems": added }))))
}

async fn remove_item(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let events = orders::remove_item(&state.db, &id, &item_id, None).await?;
    emit_all(&state, &events).await;
    let include = OrderInclude { table: true, items: Some(Sort::None), payments: None };
    Ok(Json(order_json(&state, &id, include).await?))
}

async fn service_fee(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let events = orders::set_service_fee(&state.db, &id, body.bool("active"), body.num("rate")).await?;
    emit_all(&state, &events).await;
    Ok(Json(order_json(&state, &id, OrderInclude::FULL).await?))
}

async fn discount(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let value = body.num("discount").unwrap_or(0.0);
    let events = orders::set_discount(&state.db, &id, value, None).await?;
    emit_all(&state, &events).await;
    Ok(Json(order_json(&state, &id, OrderInclude::FULL).await?))
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<String>,
    date: Option<String>,
}

/// Comandas pagas/fechadas, mais recentes primeiro; `date` (AAAA-MM-DD) filtra o dia em UTC, como o Node.
async fn history(State(state): State<AppState>, Query(q): Query<HistoryQuery>) -> ApiResult<Json<Value>> {
    let limit = q.limit.as_deref().and_then(|l| l.trim().parse::<f64>().ok()).filter(|l| *l > 0.0).unwrap_or(100.0) as i64;
    let mut filter = String::from(r#"WHERE "status" IN ('PAID', 'CLOSED')"#);
    let mut binds = Vec::new();
    if let Some(day) = q.date.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) {
        let start = Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default());
        let end = start + chrono::Duration::milliseconds(86_400_000 - 1);
        filter.push_str(r#" AND "closedAt" >= ? AND "closedAt" <= ?"#);
        binds.push(start.timestamp_millis().to_string());
        binds.push(end.timestamp_millis().to_string());
    }
    let sql = format!(r#"SELECT * FROM "Order" {filter} ORDER BY "closedAt" DESC LIMIT {limit}"#);
    let mut query = sqlx::query::<sqlx::Sqlite>(&sql);
    for b in &binds {
        query = query.bind(b.parse::<i64>().unwrap_or_default());
    }
    let mut conn = state.db.pool().acquire().await?;
    let rows: Vec<_> = query.fetch_all(&mut *conn).await?.iter().map(prisma::row_to_obj).collect();
    let include = OrderInclude { table: true, items: Some(Sort::None), payments: Some(Sort::Desc) };
    let list = prisma::orders_with(&mut conn, rows, include).await?;
    Ok(Json(Value::Array(list.into_iter().map(Value::Object).collect())))
}

/// O garçom pede o recibo no celular e ele sai na impressora do caixa. A tela nativa imprime quando
/// registrou a impressora local; senão vai para o caixa Electron registrado há mais tempo no Socket.IO.
async fn print_receipt(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let include = OrderInclude { table: true, items: Some(Sort::Asc), payments: Some(Sort::Asc) };
    let order = order_json(&state, &id, include).await?;
    let requested_by = body.str("requestedBy").map(|r| r.chars().take(60).collect::<String>());

    if let Some(printer) = state.printer.get() {
        let (reply, answer) = oneshot::channel();
        let request = PrintRequest { order, requested_by, reply };
        if printer.send(request).await.is_err() {
            return Err(no_station());
        }
        return match tokio::time::timeout(PRINT_TIMEOUT, answer).await {
            Ok(Ok(Ok(()))) => Ok(Json(json!({ "ok": true }))),
            Ok(Ok(Err(msg))) => Err(ApiError::new(StatusCode::BAD_GATEWAY, msg)),
            Ok(Err(_)) => Err(ApiError::new(StatusCode::BAD_GATEWAY, "O computador do caixa não conseguiu imprimir o recibo.")),
            Err(_) => Err(timed_out()),
        };
    }

    let Some(sid) = state.devices.first_station() else {
        return Err(no_station());
    };
    let socket = state
        .io
        .within(PRINT_STATIONS_ROOM)
        .sockets()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.id.to_string() == sid)
        .ok_or_else(no_station)?;
    let mut payload = json!({ "order": order });
    if let Some(by) = requested_by {
        payload["requestedBy"] = json!(by);
    }
    let ack = socket
        .timeout(PRINT_TIMEOUT)
        .emit_with_ack::<_, Value>("print:receipt", &payload)
        .map_err(|_| ApiError::internal("Erro ao enviar recibo para impressão."))?
        .await;
    match ack {
        Ok(value) => {
            // O Electron responde `callback({ ok, error })`; alguns clientes mandam como lista de argumentos
            let value = match value {
                Value::Array(mut args) if !args.is_empty() => args.remove(0),
                other => other,
            };
            if value.get("ok").and_then(Value::as_bool) == Some(true) {
                Ok(Json(json!({ "ok": true })))
            } else {
                let msg = value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("O computador do caixa não conseguiu imprimir o recibo.");
                Err(ApiError::new(StatusCode::BAD_GATEWAY, msg))
            }
        }
        Err(socketioxide::AckError::Timeout) => Err(timed_out()),
        Err(_) => Err(ApiError::internal("Erro ao enviar recibo para impressão.")),
    }
}

fn no_station() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "Nenhum computador do caixa conectado para imprimir. Verifique se o BarERP está aberto no PC.",
    )
}

fn timed_out() -> ApiError {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "O computador do caixa não respondeu. Verifique se o BarERP está aberto e logado no PC.",
    )
}

/// Pagamentos já processados por `requestId`. O celular reenvia o POST quando o Wi-Fi oscila
/// (`fetchWithRetry`); sem isso o mesmo abatimento entraria duas vezes no caixa. Cada clique gera um id novo.
#[derive(Clone, Default)]
pub struct PayRequests(Arc<Mutex<HashMap<String, (Instant, PayCell)>>>);

/// Resposta (status e corpo) do primeiro processamento daquele `requestId`.
type PayCell = Arc<OnceCell<(StatusCode, Value)>>;

const PAY_REQUEST_TTL: Duration = Duration::from_secs(5 * 60);

impl PayRequests {
    /// A mesma célula para o mesmo id: o reenvio espera a primeira tentativa e recebe a mesma resposta.
    fn cell(&self, request_id: &str) -> PayCell {
        let mut map = self.0.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, (at, _)| now.duration_since(*at) < PAY_REQUEST_TTL);
        map.entry(request_id.to_string()).or_insert_with(|| (now, Arc::default())).1.clone()
    }
}

async fn pay(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> (StatusCode, Json<Value>) {
    let (status, value) = match body.str("requestId") {
        Some(request_id) => {
            let cell = state.pay_requests.cell(&request_id);
            cell.get_or_init(|| process_pay(&state, &id, &body)).await.clone()
        }
        None => process_pay(&state, &id, &body).await,
    };
    (status, Json(value))
}

async fn process_pay(state: &AppState, id: &str, body: &Body) -> (StatusCode, Value) {
    match pay_inner(state, id, body).await {
        Ok(value) => (StatusCode::OK, value),
        Err(err) => (err.status, err.body),
    }
}

async fn pay_inner(state: &AppState, id: &str, body: &Body) -> ApiResult<Value> {
    let payments = body
        .array("payments")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::bad_request("Nenhum pagamento fornecido"))?
        .iter()
        .filter_map(Value::as_object)
        .map(|p| orders::NewPayment {
            amount: p.get("amount").and_then(js_number).unwrap_or(0.0),
            method: p.get("method").and_then(Value::as_str).unwrap_or("CASH").to_string(),
            notes: p.get("notes").and_then(Value::as_str).filter(|n| !n.is_empty()).map(String::from),
        })
        .collect();
    let paid_items = body.array("paidItems").map(|arr| {
        arr.iter()
            .filter_map(Value::as_object)
            .map(|i| orders::PaidItemLine {
                item_id: i.get("itemId").and_then(Value::as_str).unwrap_or_default().to_string(),
                quantity: i.get("quantity").and_then(js_number).map(|q| q as i64).unwrap_or(0),
            })
            .collect()
    });
    let input = orders::ProcessPaymentInput {
        payments,
        close_order: body.bool("closeOrder"),
        customer_id: body.str("customerId"),
        paid_items,
    };
    let (res, events) = orders::process_payment(&state.db, id, input).await?;
    emit_all(state, &events).await;

    // `include: { table: true, items: { include: { product: true } }, payments: true }`
    let order = order_json(state, id, OrderInclude::FULL).await?;
    Ok(json!({ "success": true, "order": order, "isFullyPaid": res.is_fully_paid }))
}

async fn quick_sale(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    if body.array("items").is_none_or(Vec::is_empty) {
        return Err(ApiError::bad_request("Nenhum item informado para a venda rápida"));
    }
    if body.get("payment").and_then(|p| p.get("method")).and_then(Value::as_str).is_none_or(str::is_empty) {
        return Err(ApiError::bad_request("Forma de pagamento não informada"));
    }
    let input = orders::QuickSaleInput {
        items: parse_lines(body.array("items").map(Vec::as_slice).unwrap_or_default()),
        payment: serde_json::from_value(body.get("payment").cloned().unwrap_or_default())
            .map_err(|_| ApiError::bad_request("Forma de pagamento inválida"))?,
        customer_name: body.str("customerName"),
        discount: body.num("discount"),
        expected_total: None,
    };
    let (res, events) = orders::quick_sale(&state.db, input, Local::now().naive_local()).await?;
    emit_all(&state, &events).await;

    // `include: { items: { include: { product: true } }, payments: true }`
    let include = OrderInclude { table: false, items: Some(Sort::None), payments: Some(Sort::None) };
    let order = order_json(&state, &res.order.head.id, include).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "success": true,
            "order": order,
            "change": res.change,
            "message": "Venda de balcão concluída com sucesso!"
        })),
    ))
}

/// Baixa como perda (calote/evasão): fecha sem pagamento, libera a mesa e registra na auditoria.
async fn loss(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (lost_amount, events) = barerp_db::order_history::mark_as_loss(&state.db, &id, None).await?;
    emit_all(&state, &events).await;
    Ok(Json(json!({ "success": true, "message": "Baixa de perda registrada com sucesso.", "lostAmount": lost_amount })))
}
