//! Linha `DigitalMenuSettings` e a resposta de status da tela.

use chrono::{DateTime, Utc};
use serde::Serialize;

pub const DEFAULT_MENU_SITE_URL: &str = "https://cardapio-barerp.netlify.app";

/// Espelho da tabela `DigitalMenuSettings` (linha `id = 'default'`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DigitalMenuSettings {
    pub enabled: bool,
    /// Vazio = site padrão do BarERP
    pub site_url: Option<String>,
    /// Identificador público da loja na URL do QR (gerado na 1ª publicação)
    pub store_id: Option<String>,
    /// Chave de publicação da loja (nunca sai do computador do bar)
    pub publish_secret: Option<String>,
    /// Nome exibido no cardápio (vazio = nome do bar)
    pub title: Option<String>,
    /// Recado no topo (ex.: horário, Wi-Fi)
    pub message: Option<String>,
    /// JSON com as categorias fora do cardápio
    pub hidden_category_ids: String,
    /// false = mostra "Esgotado"; true = esconde o produto
    pub hide_sold_out: bool,
    /// true = nunca marca esgotado (estoque não é controlado)
    pub ignore_stock: bool,
    pub last_published_at: Option<DateTime<Utc>>,
    pub last_hash: Option<String>,
    pub last_error: Option<String>,
}

/// Endereço do site sem `/` no final (vazio = site padrão).
pub fn effective_site_url(settings: &DigitalMenuSettings) -> String {
    let url = settings
        .site_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MENU_SITE_URL);
    url.trim_end_matches('/').to_string()
}

pub(crate) fn parse_ids(json: &str) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(if json.is_empty() { "[]" } else { json }) {
        Ok(serde_json::Value::Array(list)) => list.iter().map(js_string).collect(),
        _ => Vec::new(),
    }
}

/// `String(v)` do JS para os valores que aparecem nas configurações.
pub(crate) fn js_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => (f as i64).to_string(),
            _ => n.to_string(),
        },
        serde_json::Value::Array(a) => a.iter().map(js_string).collect::<Vec<_>>().join(","),
        serde_json::Value::Object(_) => "[object Object]".into(),
        other => other.to_string(),
    }
}

/// https obrigatório; http só no próprio computador (testes). Mesmo critério do `SITE_URL_RE` do Node.
pub(crate) fn valid_site_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let (host_port, local_only) = if let Some(rest) = lower.strip_prefix("https://") {
        (rest, false)
    } else if let Some(rest) = lower.strip_prefix("http://") {
        (rest, true)
    } else {
        return false;
    };
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (host_port, None),
    };
    if port.is_some_and(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit())) {
        return false;
    }
    if local_only {
        host == "localhost" || host == "127.0.0.1"
    } else {
        !host.is_empty()
            && host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
    }
}

/// Resposta de `GET /digital-menu` (mesmos campos do Node).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalMenuStatus {
    pub enabled: bool,
    pub site_url: String,
    pub custom_site_url: String,
    pub store_id: Option<String>,
    pub menu_url: Option<String>,
    pub title: String,
    pub default_title: String,
    pub message: String,
    pub hidden_category_ids: Vec<String>,
    pub hide_sold_out: bool,
    pub ignore_stock: bool,
    pub last_published_at: Option<String>,
    pub last_error: Option<String>,
    pub publishing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endereco_do_site() {
        assert!(valid_site_url("https://cardapio-barerp.netlify.app"));
        assert!(valid_site_url("HTTPS://Meu-Bar.netlify.app:443"));
        assert!(valid_site_url("http://localhost:8888"));
        assert!(valid_site_url("http://127.0.0.1"));
        assert!(!valid_site_url("http://meubar.com"));
        assert!(!valid_site_url("https://meubar.com/caminho"));
        assert!(!valid_site_url("https://"));
        assert!(!valid_site_url("ftp://x.com"));
        assert!(!valid_site_url("https://x.com:"));
    }

    #[test]
    fn site_padrao_sem_barra() {
        let mut s = DigitalMenuSettings::default();
        assert_eq!(effective_site_url(&s), DEFAULT_MENU_SITE_URL);
        s.site_url = Some("https://meu.netlify.app///".into());
        assert_eq!(effective_site_url(&s), "https://meu.netlify.app");
    }

    #[test]
    fn ids_escondidos() {
        assert_eq!(parse_ids(r#"["a", 2]"#), vec!["a", "2"]);
        assert!(parse_ids("lixo").is_empty());
        assert!(parse_ids("").is_empty());
        assert!(parse_ids("{}").is_empty());
    }
}
