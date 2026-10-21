//! `/api/dashboard` e `/api/audit-logs`. Tradução de `server/src/routes/dashboard.ts` e `auditLogs.ts`
//! (as regras ficam em `barerp_db::dashboard`, as mesmas da Retaguarda do PC).

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use barerp_db::dashboard;

use crate::error::ApiResult;
use crate::AppState;

pub fn dashboard_router() -> Router<AppState> {
    Router::new().route("/", get(get_dashboard))
}

pub fn audit_router() -> Router<AppState> {
    Router::new().route("/", get(audit_logs))
}

#[derive(Deserialize)]
struct DashboardQuery {
    date: Option<String>,
}

async fn get_dashboard(State(state): State<AppState>, Query(q): Query<DashboardQuery>) -> ApiResult<Json<Value>> {
    // `?date=AAAA-MM-DD` (dia de movimento local, das 6h às 5h59); sem data ou inválida = hoje
    let day = q
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or_else(barerp_core::business_day::today);
    let d = dashboard::dashboard(&state.db, day).await?;
    // Objeto { "PIX": 50, "CASH": 30 } como o Node (na ordem em que as formas apareceram)
    let breakdown: Map<String, Value> = d.payment_methods_breakdown.iter().map(|(m, v)| (m.clone(), json!(v))).collect();
    Ok(Json(json!({
        "todayRevenue": d.today_revenue,
        "ordersCompletedToday": d.orders_completed_today,
        "occupiedTables": d.occupied_tables,
        "totalTables": d.total_tables,
        "occupancyRate": d.occupancy_rate,
        "averageTicket": d.average_ticket,
        "topProducts": d.top_products,
        "paymentMethodsBreakdown": breakdown,
        "salesByHour": d.sales_by_hour,
        "waiterPerformance": d.waiter_performance,
        "lowMarginProducts": d.low_margin_products,
    })))
}

async fn audit_logs(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(dashboard::audit_logs(&state.db).await?)))
}
