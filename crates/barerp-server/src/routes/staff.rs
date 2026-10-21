//! `/api/staff`: lista pública da tela de login, login por PIN e o cadastro de funcionários (ManageStaffModal).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use barerp_db::staff::{self, LoginError, StaffInput};
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::http::Body;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public-list", get(public_list))
        .route("/login", post(login))
        .route("/", get(list).post(create))
        .route("/:id", put(update).delete(remove))
}

fn permissions(body: &Body) -> Option<Vec<String>> {
    body.array("permissions").map(|list| list.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
}

async fn list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let list = staff::list_all(&state.db)
        .await
        .map_err(|_| ApiError::internal("Erro ao buscar equipe de funcionários"))?;
    Ok(Json(json!(list)))
}

async fn create(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    let input = StaffInput {
        name: body.str("name").unwrap_or_default(),
        role: body.str("role").unwrap_or_else(|| "OPERADOR".into()),
        password: body.str("password").unwrap_or_default(),
        permissions: permissions(&body).unwrap_or_else(|| vec!["tables".into(), "cash".into()]),
        active: body.bool("active").unwrap_or(true),
    };
    Ok((StatusCode::CREATED, Json(json!(staff::create_staff(&state.db, input).await?))))
}

/// Só muda o que veio no corpo, como o Prisma `update` do Node.
async fn update(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let current = staff::list_all(&state.db)
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::not_found("Funcionário não encontrado"))?;
    let input = StaffInput {
        name: body.str("name").filter(|n| !n.trim().is_empty()).unwrap_or(current.name),
        role: body.str("role").unwrap_or(current.role),
        password: body.str("password").unwrap_or_default(),
        permissions: permissions(&body).unwrap_or(current.permissions),
        active: body.bool("active").unwrap_or(current.active),
    };
    staff::update_staff(&state.db, &id, input).await?;
    let updated = staff::list_all(&state.db).await?.into_iter().find(|s| s.id == id);
    Ok(Json(json!(updated)))
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    staff::delete_staff(&state.db, &id).await?;
    Ok(Json(json!({ "success": true, "message": "Funcionário removido com sucesso" })))
}

async fn public_list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let list = staff::public_list(&state.db)
        .await
        .map_err(|_| ApiError::internal("Erro ao buscar funcionários"))?;
    Ok(Json(json!(list.iter().map(|s| json!({ "id": s.id, "name": s.name, "role": s.role })).collect::<Vec<_>>())))
}

async fn login(State(state): State<AppState>, body: Body) -> ApiResult<Json<Value>> {
    let user_id = body.str("userId").unwrap_or_default();
    let password = body.str("password").unwrap_or_default();
    match staff::login(&state.db, &user_id, &password).await {
        Ok(user) => Ok(Json(json!({
            "success": true,
            "user": { "id": user.id, "name": user.name, "role": user.role, "permissions": user.permissions },
        }))),
        Err(err) => {
            let msg = err.to_string();
            Err(match err {
                LoginError::Missing => ApiError::bad_request(msg),
                LoginError::NotFound => ApiError::not_found(msg),
                LoginError::Inactive => ApiError::new(StatusCode::FORBIDDEN, msg),
                LoginError::LockedOut(secs) => ApiError::with_body(
                    StatusCode::TOO_MANY_REQUESTS,
                    json!({ "error": msg, "lockedOut": true, "remainingSeconds": secs }),
                ),
                LoginError::JustLocked => ApiError::with_body(
                    StatusCode::TOO_MANY_REQUESTS,
                    json!({ "error": msg, "lockedOut": true, "remainingSeconds": 15 }),
                ),
                LoginError::WrongPassword(attempts) => ApiError::with_body(
                    StatusCode::UNAUTHORIZED,
                    json!({ "error": msg, "attempts": attempts, "maxAttempts": 5, "lockedOut": false }),
                ),
                LoginError::Internal => ApiError::internal(msg),
            })
        }
    }
}
