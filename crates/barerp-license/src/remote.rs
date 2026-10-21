//! Painel de licenças da Software House (Render): `GET <url>/api/v1/licenses/check/<machineId>`.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// Resposta do painel (os campos que o Node usa).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub valid: bool,
    pub status: Option<String>,
    pub expires_at: Option<String>,
    pub client_name: Option<String>,
    pub days_remaining: Option<i64>,
    pub message: Option<String>,
    pub developer_contact: Option<Value>,
}

#[derive(Debug)]
pub enum CheckError {
    /// Sem conexão, tempo esgotado ou DNS: o Node tolera se a licença local ainda vale
    Network(String),
    /// O painel respondeu, mas não com o JSON esperado
    BadResponse(String),
}

/// Resposta HTTP do painel: o status (o modo em segundo plano só aceita 2xx, como o `remoteRes.ok` do Node).
pub struct Checked {
    pub ok: bool,
    pub body: CheckResponse,
}

pub fn check_url(server_url: &str, machine_id: &str) -> String {
    format!("{}/api/v1/licenses/check/{machine_id}", server_url.trim().trim_end_matches('/'))
}

/// Consulta o painel com o tempo limite do Node (6 s na tela de ativação, 4 s em segundo plano).
pub async fn check(server_url: &str, machine_id: &str, timeout: Duration) -> Result<Checked, CheckError> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| CheckError::Network(e.to_string()))?;
    let res = client
        .get(check_url(server_url, machine_id))
        .send()
        .await
        .map_err(|e| CheckError::Network(e.to_string()))?;
    let ok = res.status().is_success();
    let text = res.text().await.map_err(|e| CheckError::Network(e.to_string()))?;
    let body: CheckResponse = serde_json::from_str(&text).map_err(|_| {
        CheckError::BadResponse("O servidor de licenças respondeu algo inesperado. Tente novamente em instantes.".into())
    })?;
    Ok(Checked { ok, body })
}
