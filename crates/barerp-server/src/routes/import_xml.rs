//! `/api/import-xml/tabelas`: "Importar do Sistema Antigo" (uso exclusivo do desenvolvedor). Recebe o XML de
//! exportação de tabelas no campo `xml` (multipart, como o multer do Node); as regras ficam em `barerp_db::legado`.

use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use barerp_db::legado;

use crate::error::{ApiError, ApiResult};
use crate::events::emit_all;
use crate::AppState;

pub fn router() -> Router<AppState> {
    // Mesmo limite do multer do Node (50 MB)
    Router::new().route("/tabelas", post(tabelas)).layer(DefaultBodyLimit::max(50 * 1024 * 1024))
}

async fn tabelas(State(state): State<AppState>, mut form: Multipart) -> ApiResult<Json<Value>> {
    let mut file: Option<Vec<u8>> = None;
    while let Some(field) = form.next_field().await.map_err(|e| ApiError::bad_request(e.to_string()))? {
        if field.name() == Some("xml") {
            file = Some(field.bytes().await.map_err(|e| ApiError::bad_request(e.to_string()))?.to_vec());
        }
    }
    let Some(bytes) = file.filter(|b| !b.is_empty()) else {
        return Err(ApiError::bad_request("Nenhum arquivo enviado."));
    };
    let (resumo, events) = legado::import_tables(&state.db, &bytes)
        .await
        .map_err(|e| ApiError::internal(format!("Erro ao processar XML: {e}")))?;
    emit_all(&state, &events).await;
    Ok(Json(json!({ "ok": true, "resumo": resumo, "mensagem": resumo.message() })))
}
