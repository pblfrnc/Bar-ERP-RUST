//! Erros das rotas no formato do Node: status HTTP e corpo `{ "error": "mensagem" }`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use barerp_db::DbError;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: Value,
}

impl ApiError {
    pub fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        Self { status, body: json!({ "error": msg.into() }) }
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, msg)
    }

    /// Erro com campos extras no corpo (ex.: `lockedOut` e `remainingSeconds` do login).
    pub fn with_body(status: StatusCode, body: Value) -> Self {
        Self { status, body }
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        let status = match &err {
            DbError::NotFound(_) => StatusCode::NOT_FOUND,
            DbError::Rule(_) => StatusCode::BAD_REQUEST,
            DbError::Conflict(_) => StatusCode::CONFLICT,
            DbError::Forbidden(_) => StatusCode::FORBIDDEN,
            DbError::Sql(e) => {
                tracing::error!("erro no banco: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        Self::new(status, err.to_string())
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        DbError::from(err).into()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
