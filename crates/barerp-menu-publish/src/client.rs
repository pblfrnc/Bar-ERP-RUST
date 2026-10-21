//! Chamadas às funções do Netlify (`menu.mts` e `menu-image.mts`).

use std::error::Error as _;
use std::time::Duration;

use reqwest::{Client, Method, Response};

use crate::MenuError;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const IMAGE_UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct SiteClient {
    http: Client,
}

/// Loja no site: endereço base + identificação.
pub(crate) struct Site<'a> {
    pub url: String,
    pub store_id: &'a str,
    pub secret: &'a str,
}

impl SiteClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent(concat!("BarERP/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("cliente HTTP");
        Self { http }
    }

    /// `PUT` (publica) ou `DELETE` (tira do ar) em `/api/menu/:storeId`.
    pub async fn menu(
        &self,
        site: &Site<'_>,
        method: Method,
        body: Option<String>,
    ) -> Result<(), MenuError> {
        let url = format!("{}/api/menu/{}", site.url, site.store_id);
        let mut req = self
            .http
            .request(method, url)
            .bearer_auth(site.secret)
            .header("Content-Type", "application/json")
            .timeout(REQUEST_TIMEOUT);
        if let Some(b) = body {
            req = req.body(b);
        }
        let res = req.send().await.map_err(friendly)?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = error_field(res).await;
            return Err(MenuError::Message(message.unwrap_or_else(|| {
                format!("O site do cardápio respondeu HTTP {status}.")
            })));
        }
        Ok(())
    }

    /// Envia a foto em `/api/menu/:storeId/img/:hash`.
    pub async fn put_image(
        &self,
        site: &Site<'_>,
        hash: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> Result<(), MenuError> {
        let url = format!("{}/api/menu/{}/img/{hash}", site.url, site.store_id);
        let res = self
            .http
            .put(url)
            .bearer_auth(site.secret)
            .header("Content-Type", mime)
            .body(data)
            .timeout(IMAGE_UPLOAD_TIMEOUT)
            .send()
            .await
            .map_err(friendly)?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = error_field(res).await;
            return Err(MenuError::Message(
                message.unwrap_or_else(|| format!("HTTP {status}")),
            ));
        }
        Ok(())
    }

    /// Apaga uma foto que não é mais usada (best effort: erro é ignorado).
    pub async fn delete_image(&self, url: String, secret: String) {
        let _ = self
            .http
            .delete(url)
            .bearer_auth(secret)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
    }
}

/// `{ "error": "..." }` das funções do Netlify.
async fn error_field(res: Response) -> Option<String> {
    let v: serde_json::Value = res.json().await.ok()?;
    v.get("error")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Mesmas mensagens do `friendlyError` do Node.
pub(crate) fn friendly(err: reqwest::Error) -> MenuError {
    let text = if err.is_timeout() {
        "O site do cardápio não respondeu. Verifique a internet do computador.".to_string()
    } else if is_dns(&err) {
        "Sem internet no computador: o cardápio será publicado quando a conexão voltar.".to_string()
    } else if err.is_connect() || err.is_request() {
        "Não foi possível conectar ao site do cardápio. Será tentado de novo em alguns minutos."
            .to_string()
    } else {
        err.to_string()
    };
    MenuError::Message(text)
}

/// Falha de DNS (equivalente ao ENOTFOUND / EAI_AGAIN do Node).
fn is_dns(err: &reqwest::Error) -> bool {
    let mut source = err.source();
    while let Some(e) = source {
        let s = e.to_string().to_lowercase();
        if s.contains("dns error")
            || s.contains("failed to lookup address")
            || s.contains("name or service not known")
        {
            return true;
        }
        source = e.source();
    }
    false
}
