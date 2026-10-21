//! `/api/settings`: licença (status, sincronização com o painel, chave offline), margem padrão e impressoras.
//! Tradução de `server/src/routes/settings.ts`. Os APKs usam o GET para travar quando a licença vence e a tela de
//! ativação deles chama `license/sync-remote` e `license/offline` neste servidor (o do PC do caixa).

use std::sync::OnceLock;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use barerp_db::printer::{self, PrinterSettings};

use crate::error::{ApiError, ApiResult};
use crate::http::js_number;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/license/sync-remote", post(sync_remote))
        .route("/license/offline", post(offline))
        .route("/license", post(legacy))
        .route("/printer", get(get_printer).post(save_printer))
}

/// ID desta máquina, calculado uma vez (roda o `REG QUERY` no Windows).
async fn machine_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    if let Some(id) = ID.get() {
        return id.clone();
    }
    let id = tokio::task::spawn_blocking(barerp_license::machine::machine_id)
        .await
        .unwrap_or_else(|_| barerp_license::machine::UNKNOWN.to_string());
    ID.get_or_init(|| id).clone()
}

fn iso(at: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    at.map(|d| json!(d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))).unwrap_or(Value::Null)
}

async fn get_settings(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mid = machine_id().await;
    let st = barerp_license::status(&state.db, &mid).await.map_err(|e| ApiError::internal(e.to_string()))?;
    // Revalida no painel em segundo plano (a cada 2 min), como o Node
    if st.should_check_remote {
        let (db, mid) = (state.db.clone(), mid.clone());
        tokio::spawn(async move {
            let _ = barerp_license::background_check(&db, &mid).await;
        });
    }
    Ok(Json(json!({
        "id": "default",
        "restaurantName": st.restaurant_name,
        "machineId": st.machine_id,
        "isLicensed": st.is_licensed,
        "licenseStatus": st.license_status,
        "expiresAt": iso(st.expires_at),
        "lastVerifiedAt": iso(st.last_verified_at),
        "clientName": st.client_name,
        "licenseServerUrl": st.license_server_url,
        "defaultProfitMargin": st.default_profit_margin,
        "daysRemaining": st.days_remaining,
        "isExpiringSoon": st.is_expiring_soon,
        "developerContact": st.developer_contact,
    })))
}

async fn put_settings(State(state): State<AppState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    if let Some(name) = body.get("restaurantName").filter(|v| !v.is_null()) {
        let name = name.as_str().map(str::to_string).unwrap_or_else(|| name.to_string());
        barerp_db::retaguarda::save_system_settings(&state.db, &name).await?;
    }
    if let Some(margin) = body.get("defaultProfitMargin").filter(|v| !v.is_null()) {
        barerp_db::retaguarda::set_default_profit_margin(&state.db, js_number(margin).unwrap_or(50.0)).await?;
    }
    let mid = machine_id().await;
    let st = barerp_license::status(&state.db, &mid).await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({
        "success": true,
        "settings": { "restaurantName": st.restaurant_name, "defaultProfitMargin": st.default_profit_margin },
    })))
}

async fn sync_remote(State(state): State<AppState>, body: Option<Json<Value>>) -> ApiResult<(StatusCode, Json<Value>)> {
    let server_url = body.as_ref().and_then(|Json(b)| b.get("serverUrl")).and_then(Value::as_str).map(str::to_string);
    let mid = machine_id().await;
    match barerp_license::sync_remote(&state.db, &mid, server_url.as_deref()).await {
        Ok(out) => {
            let st = barerp_license::status(&state.db, &mid).await.map_err(|e| ApiError::internal(e.to_string()))?;
            let body = if out.activated {
                json!({
                    "success": true,
                    "activated": true,
                    "message": out.message,
                    "expiresAt": iso(st.expires_at),
                    "daysRemaining": out.days_remaining.unwrap_or(st.days_remaining),
                    "clientName": st.client_name,
                })
            } else {
                json!({
                    "success": true,
                    "offlineFallback": true,
                    "message": out.message,
                    "isLicensed": true,
                    "expiresAt": iso(st.expires_at),
                    "daysRemaining": out.days_remaining.unwrap_or(st.days_remaining),
                })
            };
            Ok((StatusCode::OK, Json(body)))
        }
        // O APK mostra `message` na tela de ativação
        Err(e) => Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "success": false, "activated": false, "message": e.to_string(), "error": e.to_string() })),
        )),
    }
}

async fn offline(State(state): State<AppState>, body: Option<Json<Value>>) -> ApiResult<(StatusCode, Json<Value>)> {
    let key = body.as_ref().and_then(|Json(b)| b.get("key")).and_then(Value::as_str).unwrap_or("").to_string();
    if key.trim().is_empty() {
        return Ok((StatusCode::BAD_REQUEST, Json(json!({ "success": false, "error": "Chave de ativação é obrigatória." }))));
    }
    let mid = machine_id().await;
    match barerp_license::activate_offline(&state.db, &mid, &key).await {
        Ok(message) => {
            let st = barerp_license::status(&state.db, &mid).await.map_err(|e| ApiError::internal(e.to_string()))?;
            Ok((
                StatusCode::OK,
                Json(json!({ "success": true, "message": message, "expiresAt": iso(st.expires_at), "daysRemaining": st.days_remaining })),
            ))
        }
        Err(e) => Ok((StatusCode::BAD_REQUEST, Json(json!({ "success": false, "error": e.to_string() })))),
    }
}

/// Ativação legada (e-mail + chave permanente), como o POST `/license` do Node.
async fn legacy(State(state): State<AppState>, body: Option<Json<Value>>) -> ApiResult<(StatusCode, Json<Value>)> {
    let field = |k: &str| body.as_ref().and_then(|Json(b)| b.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
    let (email, key) = (field("email"), field("key"));
    if email.is_empty() || key.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": "Email e chave são obrigatórios." }))));
    }
    let mid = machine_id().await;
    let ok = barerp_license::service::activate_legacy(&state.db, &mid, &email, &key)
        .await
        .map_err(|_| ApiError::internal("Erro ao ativar licença"))?;
    if !ok {
        return Ok((StatusCode::UNAUTHORIZED, Json(json!({ "error": "Chave de licença inválida para este computador." }))));
    }
    let st = barerp_license::status(&state.db, &mid).await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((
        StatusCode::OK,
        Json(json!({ "success": true, "settings": { "restaurantName": st.restaurant_name, "machineId": st.machine_id, "isLicensed": true, "licenseStatus": "ACTIVE" } })),
    ))
}

async fn get_printer(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let s = printer::get_printer_settings(&state.db).await?;
    Ok(Json(printer_json(&s)))
}

fn printer_json(s: &PrinterSettings) -> Value {
    json!({
        "id": "default",
        "cashierPrinter": s.cashier_printer.clone().unwrap_or_default(),
        "kitchenPrinter": s.kitchen_printer.clone().unwrap_or_default(),
        "paperWidth": s.paper_width,
        "marginTop": s.margin_top,
        "marginBottom": s.margin_bottom,
        "marginLeft": s.margin_left,
        "marginRight": s.margin_right,
        "fontScale": s.font_scale,
        "qrSize": s.qr_size,
        "autoCut": s.auto_cut,
        "silentPrint": s.silent_print,
        "copies": s.copies,
        "extraFeedLines": s.extra_feed_lines,
        "printLogo": s.print_logo,
    })
}

async fn save_printer(State(state): State<AppState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let text = |k: &str| body.get(k).and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let num = |k: &str, default: f64| body.get(k).filter(|v| !v.is_null()).and_then(js_number).unwrap_or(default);
    let flag = |k: &str| body.get(k) != Some(&Value::Bool(false));
    let current = printer::get_printer_settings(&state.db).await?;
    let settings = PrinterSettings {
        cashier_printer: text("cashierPrinter"),
        kitchen_printer: text("kitchenPrinter"),
        paper_width: Some(num("paperWidth", 80.0)).filter(|v| *v != 0.0).unwrap_or(80.0),
        margin_top: num("marginTop", 2.0),
        margin_bottom: num("marginBottom", 12.0),
        margin_left: num("marginLeft", 1.0),
        margin_right: num("marginRight", 1.0),
        font_scale: num("fontScale", 100.0),
        qr_size: num("qrSize", 100.0),
        auto_cut: flag("autoCut"),
        silent_print: flag("silentPrint"),
        copies: (num("copies", 1.0) as i64).max(1),
        extra_feed_lines: (num("extraFeedLines", 3.0) as i64).max(0),
        print_logo: flag("printLogo"),
        ..current
    };
    let saved = printer::save_printer_settings(&state.db, &settings).await?;
    Ok(Json(json!({ "success": true, "settings": printer_json(&saved) })))
}
