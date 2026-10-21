//! `/api/system`: backup automático e cardápio digital. Tradução de `server/src/routes/system.ts`.
//!
//! Usa os mesmos serviços que o app nativo roda (com os agendadores), entregues por
//! `ServerHandle::set_system_services`; sem eles (testes, servidor avulso) cria serviços sem agendador.

use std::sync::{Arc, OnceLock};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};

use barerp_backup::{BackupService, BackupSettingsInput};
use barerp_menu_publish::db_store::DbMenuStore;
use barerp_menu_publish::MenuPublisher;

use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// Backup e publicador do cardápio compartilhados com o app.
#[derive(Clone)]
pub struct SystemServices {
    pub backup: Option<BackupService>,
    pub menu: MenuPublisher<DbMenuStore>,
}

pub(crate) type SharedSystem = Arc<OnceLock<SystemServices>>;

async fn services(state: &AppState) -> SystemServices {
    if let Some(s) = state.system.get() {
        return s.clone();
    }
    let backup = BackupService::from_pool(state.db.pool().clone()).await.ok();
    let created = SystemServices { backup, menu: MenuPublisher::new(DbMenuStore::new(state.db.clone())) };
    state.system.get_or_init(|| created).clone()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/backup-status", get(backup_status))
        .route("/backup-settings", put(backup_settings))
        .route("/backup-now", post(backup_now))
        .route("/digital-menu", get(menu_status).put(menu_save))
        .route("/digital-menu/publish", post(menu_publish))
}

async fn backup_service(state: &AppState) -> ApiResult<BackupService> {
    services(state)
        .await
        .backup
        .ok_or_else(|| ApiError::internal("Não foi possível localizar o arquivo do banco de dados."))
}

async fn backup_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(backup_service(&state).await?.status())))
}

async fn backup_settings(State(state): State<AppState>, body: Option<Json<Value>>) -> ApiResult<Json<Value>> {
    let svc = backup_service(&state).await?;
    let input: BackupSettingsInput =
        body.and_then(|Json(v)| serde_json::from_value(v).ok()).unwrap_or_default();
    let settings = svc.save_settings(&input).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(json!({ "success": true, "settings": settings })))
}

async fn backup_now(State(state): State<AppState>) -> ApiResult<(StatusCode, Json<Value>)> {
    let result = backup_service(&state).await?.run("manual").await;
    let status = if result.error.is_some() { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::OK };
    Ok((status, Json(json!(result))))
}

async fn menu_status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let st = services(&state).await.menu.status().await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!(st)))
}

async fn menu_save(State(state): State<AppState>, body: Option<Json<Value>>) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let res = services(&state).await.menu.save_settings(&body).await.map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(json!(res)))
}

async fn menu_publish(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let svc = services(&state).await;
    svc.menu.publish(true).await.map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let st = svc.menu.status().await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!(st)))
}
