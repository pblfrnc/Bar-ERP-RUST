//! Rotas `/api/fiscal` e o agendador das NF-e recebidas. Tradução de `server/src/routes/fiscal.ts` e
//! `server/src/services/nfeRecebidasScheduler.ts`: mesmos caminhos, corpos e mensagens do Node.
//!
//! A conversa com a Focus NFe fica no `barerp-fiscal` e o banco no `barerp_db::notas`/`importacao`; aqui só
//! a cola HTTP. Só `autorizado` é sucesso, e nenhum dado fiscal é inventado (sem XML, valor ou data falsos).

mod agendador;
mod config;
mod contador;
mod emissao;
mod recebidas;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use barerp_db::notas::{self, FiscalSettings};
use barerp_db::Db;
use barerp_fiscal::{Ambiente, Emitente, FiscalError, FocusClient};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::error::{ApiError, ApiResult};

pub use agendador::iniciar_agendador;

/// Estado das rotas fiscais, independente do `AppState` do servidor.
#[derive(Clone)]
pub struct FiscalState {
    db: Db,
    data_dir: PathBuf,
    /// Endereço da Focus no lugar do de produção/homologação (servidor simulado nos testes).
    focus_url: Option<String>,
    brasil_api_url: String,
    ping_sefaz: Vec<String>,
    esperas: Esperas,
    /// Uma sincronização de notas recebidas por vez (agendador e botão da tela).
    sync: Arc<Mutex<()>>,
}

#[derive(Clone, Copy)]
struct Esperas {
    nfce: Duration,
    intervalo_nfe: Duration,
    limite_nfe: Duration,
    /// Entre a ciência e o download do XML no bip da chave.
    bip: Duration,
}

impl FiscalState {
    /// `data_dir` é a pasta do `bar.db`; o cofre de XML fica em `<data_dir>/xml_vault`.
    pub fn new(db: Db, data_dir: PathBuf) -> Self {
        Self {
            db,
            data_dir,
            focus_url: None,
            brasil_api_url: barerp_fiscal::publico::BRASIL_API.into(),
            ping_sefaz: barerp_fiscal::publico::PING_SEFAZ.iter().map(|s| s.to_string()).collect(),
            esperas: Esperas {
                nfce: Duration::from_millis(2500),
                intervalo_nfe: Duration::from_secs(2),
                limite_nfe: Duration::from_secs(16),
                bip: Duration::from_millis(1800),
            },
            sync: Arc::default(),
        }
    }

    /// Tudo apontando para um servidor simulado e sem esperas.
    #[cfg(test)]
    pub(crate) fn simulado(db: Db, data_dir: PathBuf, url: &str) -> Self {
        let mut s = Self::new(db, data_dir);
        s.focus_url = Some(url.to_string());
        s.brasil_api_url = url.to_string();
        s.ping_sefaz = vec![format!("{url}/ping-sefaz")];
        s.esperas = Esperas {
            nfce: Duration::ZERO,
            intervalo_nfe: Duration::ZERO,
            limite_nfe: Duration::from_millis(100),
            bip: Duration::ZERO,
        };
        s
    }

    fn cofre(&self) -> PathBuf {
        self.data_dir.join("xml_vault")
    }

    async fn config(&self) -> ApiResult<FiscalSettings> {
        Ok(notas::ler_config_fiscal(&self.db).await?.unwrap_or_default())
    }

    /// Cliente da Focus com o token e o ambiente da configuração; `None` sem token.
    fn focus(&self, cfg: &FiscalSettings) -> Option<FocusClient> {
        let token = cfg.token()?;
        self.focus_com(token, cfg.environment.as_deref().unwrap_or(""), cfg.cnpj.as_deref().unwrap_or(""))
    }

    fn focus_com(&self, token: &str, environment: &str, cnpj: &str) -> Option<FocusClient> {
        let ambiente = Ambiente::from_setting(environment);
        let base = self.focus_url.as_deref().unwrap_or(ambiente.base_url());
        let e = self.esperas;
        FocusClient::com_base_url(base, ambiente, token, cnpj)
            .ok()
            .map(|c| c.com_esperas(e.nfce, e.intervalo_nfe, e.limite_nfe))
    }

    /// Cliente da Focus ou 400 com a mensagem da rota.
    fn focus_ou(&self, cfg: &FiscalSettings, msg: &str) -> ApiResult<FocusClient> {
        self.focus(cfg).ok_or_else(|| ApiError::bad_request(msg))
    }
}

/// Monta o roteador; o servidor encaixa em `/api/fiscal`.
pub fn router(state: FiscalState) -> Router {
    Router::new()
        .route("/import-xml", post(recebidas::import_xml))
        .route("/validate-api", get(config::validate_api))
        .route("/diagnose-focus", get(config::diagnose_focus))
        .route("/apply-import", post(recebidas::apply_import))
        .route("/settings", get(config::get_settings).put(config::put_settings))
        .route("/establishment", get(config::establishment))
        .route("/upload-cert", post(config::upload_cert))
        .route("/cancel-nfce", post(emissao::cancel_nfce))
        .route("/cancelable-notes", get(emissao::cancelable_nfce))
        .route("/nfe/reprint/:numero", get(emissao::nfe_reprint))
        .route("/recent-notes", get(emissao::recent_notes))
        .route("/reprint-by-number/:numero", get(emissao::reprint_by_number))
        .route("/danfe/:referencia", get(emissao::danfe))
        .route("/cancel-nfe", post(emissao::cancel_nfe))
        .route("/nfe/carta-correcao", post(emissao::carta_correcao))
        .route("/nfe/cancelable-notes", get(emissao::cancelable_nfe))
        .route("/emit-nfce", post(emissao::emit_nfce))
        .route("/emit-nfe", post(emissao::emit_nfe))
        .route("/send-email", post(emissao::send_email))
        .route("/bip-chave", post(recebidas::bip_chave))
        .route("/notas-pendentes", get(recebidas::notas_pendentes))
        .route("/notas-pendentes/:chave/receber", post(recebidas::receber))
        .route("/notas-pendentes/:chave/finalizar", post(recebidas::finalizar))
        .route("/notas-pendentes/:chave/reabrir", post(recebidas::reabrir))
        .route("/notas-recebidas", get(recebidas::notas_recebidas))
        .route("/notas-recebidas/:chave/sync", post(recebidas::sync_nota))
        .route("/notas-recebidas/:chave/xml", get(recebidas::xml_nota))
        .route("/parsed-by-chave/:chave", get(recebidas::parsed_by_chave))
        .route("/sync-nfe-recebidas", get(recebidas::sync_todas))
        .route("/reprint/:ref", get(emissao::reprint_ref))
        .route("/month-summary", get(contador::month_summary))
        .route("/export-month", get(contador::export_month))
        .route("/focus-backups", get(config::focus_backups))
        .route("/ncm/:code", get(config::ncm))
        .route("/ncms", get(config::pesquisar_ncms))
        .route("/sefaz-status", get(config::sefaz_status))
        .route("/inutilizar-numeracao", post(emissao::inutilizar_nfe))
        .route("/inutilizar-numeracao-nfce", post(emissao::inutilizar_nfce))
        .route("/consulta-cnpj/:cnpj", get(config::consulta_cnpj))
        // além do Node: XML autorizado de uma nota emitida, para o app imprimir o DANFE NFC-e em ESC/POS
        .route("/xml-emitida/:referencia", get(emissao::xml_emitida))
        // XML e certificado A1 (o multer do Node aceitava até 10 MB)
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .with_state(state)
}

// ============================================================
// Chamada dentro do processo (app nativo)
// ============================================================

/// Corpo de uma chamada feita pelo app nativo.
#[derive(Debug, Clone)]
pub enum Corpo {
    Nada,
    Json(Value),
    /// Formulário com arquivo (`/import-xml`, `/settings`, `/upload-cert`).
    Multipart(Vec<CampoMultipart>),
}

#[derive(Debug, Clone)]
pub struct CampoMultipart {
    pub nome: String,
    /// Nome do arquivo, quando o campo é um arquivo.
    pub arquivo: Option<String>,
    pub conteudo: Vec<u8>,
}

impl CampoMultipart {
    pub fn texto(nome: &str, valor: &str) -> Self {
        Self { nome: nome.into(), arquivo: None, conteudo: valor.as_bytes().to_vec() }
    }

    pub fn arquivo(nome: &str, arquivo: &str, conteudo: Vec<u8>) -> Self {
        Self { nome: nome.into(), arquivo: Some(arquivo.into()), conteudo }
    }
}

/// Resposta de [`chamar`]: a mesma que os celulares recebem pela rede.
#[derive(Debug, Clone)]
pub struct RespostaFiscal {
    pub status: u16,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl RespostaFiscal {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Corpo JSON (`null` se não for JSON).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }

    /// Mensagem de erro do corpo (`{ "error": ... }`) ou o texto puro.
    pub fn erro(&self) -> String {
        match self.json().get("error") {
            Some(Value::String(e)) => e.clone(),
            _ => String::from_utf8_lossy(&self.bytes).into_owned(),
        }
    }
}

/// Chama uma rota fiscal sem passar pela rede (o app nativo usa as mesmas rotas que os celulares).
/// `caminho` é o da rota sem o prefixo `/api/fiscal`, com a query, ex.: `"/recent-notes?tipo=nfce"`.
pub async fn chamar(state: &FiscalState, metodo: &str, caminho: &str, corpo: Corpo) -> RespostaFiscal {
    use tower::Service;
    const LIMITE: &str = "----barerp-app";
    let req = axum::http::Request::builder().method(metodo).uri(caminho).header("host", "127.0.0.1:3001");
    let req = match corpo {
        Corpo::Nada => req.body(axum::body::Body::empty()),
        Corpo::Json(v) => req.header("content-type", "application/json").body(axum::body::Body::from(v.to_string())),
        Corpo::Multipart(campos) => {
            let mut b = Vec::new();
            for c in campos {
                b.extend(format!("--{LIMITE}\r\nContent-Disposition: form-data; name=\"{}\"", c.nome).as_bytes());
                if let Some(a) = &c.arquivo {
                    b.extend(format!("; filename=\"{a}\"\r\nContent-Type: application/octet-stream").as_bytes());
                }
                b.extend(b"\r\n\r\n");
                b.extend(c.conteudo);
                b.extend(b"\r\n");
            }
            b.extend(format!("--{LIMITE}--\r\n").as_bytes());
            req.header("content-type", format!("multipart/form-data; boundary={LIMITE}")).body(axum::body::Body::from(b))
        }
    };
    let falha = |msg: String| RespostaFiscal {
        status: 500,
        content_type: "application/json".into(),
        bytes: json!({ "error": msg }).to_string().into_bytes(),
    };
    let req = match req {
        Ok(r) => r,
        Err(e) => return falha(format!("Requisição inválida: {e}")),
    };
    let mut svc = router(state.clone());
    if let Err(e) = std::future::poll_fn(|cx| <Router as Service<axum::extract::Request>>::poll_ready(&mut svc, cx)).await {
        return falha(e.to_string());
    }
    let res = match svc.call(req).await {
        Ok(r) => r,
        Err(e) => return falha(e.to_string()),
    };
    let status = res.status().as_u16();
    let content_type =
        res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    match axum::body::to_bytes(res.into_body(), usize::MAX).await {
        Ok(b) => RespostaFiscal { status, content_type, bytes: b.to_vec() },
        Err(e) => falha(e.to_string()),
    }
}

// ============================================================
// Apoio comum às rotas
// ============================================================

/// Erro do `barerp-fiscal` como o Node responde na emissão: 400 com a mensagem, 500 em falha de rede.
fn erro_fiscal(e: FiscalError) -> ApiError {
    match e {
        FiscalError::Rede(_) => ApiError::internal(e.to_string()),
        outro => ApiError::bad_request(outro.to_string()),
    }
}

/// Erro com o status HTTP da Focus repassado e os detalhes, como no cancelamento e na inutilização.
fn erro_repassado(e: FiscalError, prefixo: &str) -> ApiError {
    match e {
        FiscalError::Focus { http_status, mensagem, detalhes } => ApiError::with_body(
            StatusCode::from_u16(http_status).unwrap_or(StatusCode::BAD_REQUEST),
            json!({ "error": format!("{prefixo}{mensagem}"), "details": detalhes }),
        ),
        FiscalError::Sefaz { mensagem, detalhes, .. } => ApiError::with_body(
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("{prefixo}{mensagem}"), "details": detalhes }),
        ),
        outro => erro_fiscal(outro),
    }
}

fn emitente(cfg: &FiscalSettings) -> Emitente {
    let e = cfg.emitente();
    Emitente {
        cnpj: e.cnpj,
        crt: e.crt,
        serie_nfce: e.serie_nfce,
        serie_nfe: e.serie_nfe,
        cep: e.cep,
        logradouro: e.logradouro,
        numero: e.numero,
        bairro: e.bairro,
        municipio: e.municipio,
        uf: e.uf,
    }
}

/// `encodeURIComponent`.
fn uri_seg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// URL do DANFE servido por este servidor, com o host que o aparelho usou (`req.get('host')` do Node).
fn danfe_url(headers: &HeaderMap, referencia: &str) -> String {
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("localhost:3001");
    format!("http://{host}/api/fiscal/danfe/{}", uri_seg(referencia))
}

/// Texto de um campo do corpo (string, número ou ausente).
fn txt(v: &Value, campo: &str) -> String {
    match v.get(campo) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn opt(v: &Value, campo: &str) -> Option<String> {
    Some(txt(v, campo)).filter(|s| !s.is_empty())
}

/// `Number(x) || 0`.
fn num(v: &Value, campo: &str) -> f64 {
    match v.get(campo) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn so_digitos(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn agora_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Baixa o XML autorizado e guarda no cofre de SAÍDA, sem segurar a resposta (falha não trava a venda: a
/// Focus também guarda o XML).
fn arquivar_saida_em_segundo_plano(state: &FiscalState, focus: &FocusClient, caminho_xml: Option<String>, chave: String) {
    let Some(caminho) = caminho_xml.filter(|c| !c.is_empty()) else {
        return;
    };
    let (focus, cofre) = (focus.clone(), state.cofre());
    tokio::spawn(async move {
        match focus.baixar(&caminho).await {
            Ok(a) => {
                let xml = String::from_utf8_lossy(&a.bytes).into_owned();
                if xml.contains("<nfeProc") || xml.contains("<NFe") {
                    if let Err(e) = notas::arquivar_xml(&cofre, notas::LadoCofre::Saida, &chave, &xml) {
                        tracing::warn!("cofre de XML: não gravou a nota {chave}: {e}");
                    }
                } else {
                    tracing::warn!("XML da nota {chave} ainda não disponível na Focus");
                }
            }
            Err(e) => tracing::warn!("falha ao baixar o XML da nota {chave}: {e}"),
        }
    });
}
