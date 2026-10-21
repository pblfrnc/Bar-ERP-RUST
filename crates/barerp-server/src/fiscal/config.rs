//! Configuração fiscal e consultas: dados da empresa (e cadastro dela na Focus), certificado A1, validação
//! do token, status da SEFAZ, NCM, backups da Focus e CNPJ na Receita.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use barerp_db::notas::{self, FiscalSettings};
use barerp_fiscal::{publico, Ambiente, FiscalError, Method};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{erro_fiscal, opt, so_digitos, txt, FiscalState};
use crate::error::{ApiError, ApiResult};

fn nome_ambiente(producao: bool) -> &'static str {
    if producao {
        "Produção"
    } else {
        "Homologação"
    }
}

/// Erros da Focus em uma linha (`Campo [x]: mensagem | ...`), como no cadastro da empresa.
fn erros_em_linha(d: &Value, prefixo_campo: &str, sep: &str) -> String {
    if let Some(erros) = d.get("erros").and_then(Value::as_array) {
        return erros
            .iter()
            .map(|e| {
                let campo = opt(e, "campo").map(|c| format!("{prefixo_campo}[{c}]{}", if prefixo_campo.is_empty() { " " } else { ": " }));
                let msg = opt(e, "mensagem").or_else(|| opt(e, "codigo")).unwrap_or_else(|| e.to_string());
                format!("{}{msg}", campo.unwrap_or_default())
            })
            .collect::<Vec<_>>()
            .join(sep);
    }
    opt(d, "mensagem").unwrap_or_else(|| d.to_string())
}

// ============================================================
// Configuração e dados da empresa
// ============================================================

pub(super) async fn get_settings(State(st): State<FiscalState>) -> ApiResult<Json<FiscalSettings>> {
    Ok(Json(notas::config_fiscal(&st.db).await?))
}

pub(super) async fn establishment(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    let cfg = st.config().await?;
    let vazio = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
    Ok(Json(json!({
        "nomeFantasia": vazio(&cfg.nome_fantasia),
        "razaoSocial": vazio(&cfg.razao_social),
        "cnpj": vazio(&cfg.cnpj),
        "ie": vazio(&cfg.ie),
        "endereco": cfg.endereco_resumido(),
    })))
}

struct Envio {
    settings: Option<String>,
    cert_password: Option<String>,
    /// (nome do arquivo, conteúdo)
    certificado: Option<(String, Vec<u8>)>,
}

async fn ler_multipart(mut m: Multipart) -> ApiResult<Envio> {
    let mut e = Envio { settings: None, cert_password: None, certificado: None };
    let ruim = |err: axum::extract::multipart::MultipartError| ApiError::bad_request(format!("Envio inválido: {err}"));
    while let Some(campo) = m.next_field().await.map_err(ruim)? {
        match campo.name().unwrap_or("") {
            "settings" => e.settings = Some(campo.text().await.map_err(ruim)?),
            "certPassword" => e.cert_password = Some(campo.text().await.map_err(ruim)?),
            "certificado" => {
                let nome = campo.file_name().unwrap_or("certificado.pfx").to_string();
                e.certificado = Some((nome, campo.bytes().await.map_err(ruim)?.to_vec()));
            }
            _ => {}
        }
    }
    Ok(e)
}

/// `Number(x)` quando é número, senão o texto.
fn numero_ou_texto(s: &str) -> Value {
    match s.trim().parse::<f64>() {
        Ok(n) if n.fract() == 0.0 && n.abs() < 1e15 => json!(n as i64),
        Ok(n) => json!(n),
        Err(_) => json!(s.trim()),
    }
}

/// Corpo do cadastro da empresa na Focus (modelo Software House).
fn payload_empresa(d: &Value, certificado: Option<(&str, &str)>) -> Value {
    let t = |k: &str| txt(d, k).trim().to_string();
    let csc = t("cscSecret");
    let csc_id = t("cscId");
    let mut p = Map::new();
    p.insert("nome".into(), json!(t("razaoSocial")));
    let fantasia = Some(t("nomeFantasia")).filter(|f| !f.is_empty()).unwrap_or_else(|| t("razaoSocial"));
    p.insert("nome_fantasia".into(), json!(fantasia));
    p.insert("cnpj".into(), json!(so_digitos(&t("cnpj"))));
    p.insert("regime_tributario".into(), numero_ou_texto(Some(t("crt")).filter(|c| !c.is_empty()).as_deref().unwrap_or("1")));
    p.insert("enviar_email_destinatario".into(), json!(false));
    p.insert("discrimina_impostos".into(), json!(true));
    p.insert("habilita_nfce".into(), json!(!csc.is_empty() && !csc_id.is_empty()));
    p.insert("habilita_nfe".into(), json!(true));
    if !t("ie").is_empty() {
        p.insert("inscricao_estadual".into(), json!(t("ie")));
    }
    for (de, para) in [("logradouro", "logradouro"), ("bairro", "bairro"), ("municipio", "municipio")] {
        if !t(de).is_empty() {
            p.insert(para.into(), json!(t(de)));
        }
    }
    let numero = t("numero");
    p.insert("numero".into(), if numero.is_empty() { json!("S/N") } else { numero_ou_texto(&numero) });
    let cep = so_digitos(&t("cep"));
    if !cep.is_empty() {
        p.insert("cep".into(), json!(cep));
    }
    if !t("uf").is_empty() {
        p.insert("uf".into(), json!(t("uf").to_uppercase()));
    }
    if !csc.is_empty() {
        p.insert("csc_nfce_producao".into(), json!(csc));
        p.insert("csc_nfce_homologacao".into(), json!(csc));
    }
    if !csc_id.is_empty() {
        p.insert("id_token_nfce_producao".into(), numero_ou_texto(&csc_id));
        p.insert("id_token_nfce_homologacao".into(), numero_ou_texto(&csc_id));
    }
    for (de, chave) in [
        ("serieNfce", "serie_nfce"),
        ("proximoNumeroNfce", "proximo_numero_nfce"),
        ("serieNfe", "serie_nfe"),
        ("proximoNumeroNfe", "proximo_numero_nfe"),
    ] {
        let v = t(de);
        if !v.is_empty() && v != "0" {
            p.insert(format!("{chave}_producao"), json!(v));
            p.insert(format!("{chave}_homologacao"), json!(v));
        }
    }
    if let Some((base64, senha)) = certificado {
        p.insert("arquivo_certificado_base64".into(), json!(base64));
        p.insert("senha_certificado".into(), json!(senha));
    }
    Value::Object(p)
}

/// Cria a empresa na Focus; se o CNPJ já estiver cadastrado na conta, atualiza.
async fn cadastrar_empresa(st: &FiscalState, d: &Value, certificado: Option<(&str, &str)>) -> ApiResult<()> {
    let token = txt(d, "apiToken").trim().to_string();
    let cnpj = so_digitos(&txt(d, "cnpj"));
    let environment = txt(d, "environment");
    let producao = Ambiente::from_setting(&environment) == Ambiente::Producao;
    let Some(focus) = st.focus_com(&token, &environment, &cnpj) else {
        return Err(ApiError::internal("Não foi possível preparar a conexão com a Focus NFe."));
    };
    let corpo = payload_empresa(d, certificado);
    let mut r = focus.requisitar(Method::POST, "/v2/empresas?dry_run=0", Some(&corpo)).await.map_err(erro_fiscal)?;
    let texto = r.corpo.to_string().to_lowercase();
    let ja_cadastrada = r.http_status == 400
        && ["já cadastrad", "already exist", "já existe", "empresa_ja_cadastrada", "duplicad"].iter().any(|m| texto.contains(m));
    if !r.ok() && ja_cadastrada {
        r = focus
            .requisitar(Method::PUT, &format!("/v2/empresas/{cnpj}?dry_run=0"), Some(&corpo))
            .await
            .map_err(erro_fiscal)?;
    }
    if r.ok() {
        return Ok(());
    }
    let msg = if r.http_status == 401 {
        format!(
            "Token da Focus NFe não autorizado ou inválido para este ambiente ({}). Verifique seu Token.",
            nome_ambiente(producao)
        )
    } else {
        erros_em_linha(&r.corpo, "Campo ", " | ")
    };
    Err(ApiError::bad_request(format!("Focus NFe ({}): {msg}", r.http_status)))
}

/// Junta o que a tela mandou com o que está salvo (campos ausentes não são apagados, como no Prisma).
fn mesclar(atual: &FiscalSettings, d: &Value) -> ApiResult<FiscalSettings> {
    let mut base = serde_json::to_value(atual).map_err(|e| ApiError::internal(e.to_string()))?;
    if let (Some(b), Some(novo)) = (base.as_object_mut(), d.as_object()) {
        for (k, v) in novo {
            if k == "id" || k == "updatedAt" {
                continue;
            }
            let v = match (k.as_str(), v) {
                ("proximoNumeroNfce" | "proximoNumeroNfe", Value::String(s)) => {
                    s.trim().parse::<i64>().map(Value::from).unwrap_or(Value::Null)
                }
                ("proximoNumeroNfce" | "proximoNumeroNfe", Value::Number(n)) => Value::from(n.as_f64().unwrap_or(1.0) as i64),
                (_, Value::Number(n)) => Value::String(n.to_string()),
                (_, outro) => outro.clone(),
            };
            b.insert(k.clone(), v);
        }
    }
    serde_json::from_value(base).map_err(|e| ApiError::bad_request(format!("Dados inválidos: {e}")))
}

pub(super) async fn put_settings(State(st): State<FiscalState>, m: Multipart) -> ApiResult<Json<FiscalSettings>> {
    let envio = ler_multipart(m).await?;
    let Some(texto) = envio.settings else {
        return Err(ApiError::bad_request("Dados não enviados."));
    };
    let mut d: Value = serde_json::from_str(&texto).map_err(|_| ApiError::bad_request("Dados não enviados."))?;
    if let Some(Value::String(s)) = d.get_mut("apiToken") {
        *s = s.trim().to_string();
    }
    if !txt(&d, "apiToken").is_empty() && !txt(&d, "cnpj").is_empty() {
        let cert_b64 = envio.certificado.as_ref().map(|(_, b)| base64::engine::general_purpose::STANDARD.encode(b));
        let senha = envio.cert_password.clone().filter(|s| !s.is_empty());
        let cert = cert_b64.as_deref().zip(senha.as_deref());
        cadastrar_empresa(&st, &d, cert).await?;
    }
    let atual = notas::ler_config_fiscal(&st.db).await?.unwrap_or_default();
    let novo = mesclar(&atual, &d)?;
    Ok(Json(notas::salvar_config_fiscal(&st.db, &novo).await?))
}

pub(super) async fn upload_cert(State(st): State<FiscalState>, m: Multipart) -> ApiResult<Json<Value>> {
    let envio = ler_multipart(m).await?;
    let Some((nome, bytes)) = envio.certificado else {
        return Err(ApiError::bad_request("Nenhum arquivo enviado. Selecione o arquivo .PFX do certificado."));
    };
    let senha = envio.cert_password.unwrap_or_default();
    if senha.trim().is_empty() {
        return Err(ApiError::bad_request("Senha do certificado é obrigatória."));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API não configurado. Salve as configurações primeiro.")?;
    let cnpj = so_digitos(cfg.cnpj.as_deref().unwrap_or(""));
    if cnpj.is_empty() {
        return Err(ApiError::bad_request("CNPJ não configurado. Salve as configurações da empresa primeiro."));
    }
    let corpo = json!({
        "arquivo_certificado_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "senha_certificado": senha.trim(),
    });
    let r = focus.requisitar(Method::PUT, &format!("/v2/empresas/{cnpj}"), Some(&corpo)).await.map_err(erro_fiscal)?;
    if !r.ok() {
        let msg = match r.http_status {
            401 => "Token inválido ou sem permissão para este ambiente.".to_string(),
            404 => "Empresa não encontrada na Focus NFe. Cadastre a empresa no painel de app.focusnfe.com.br primeiro.".to_string(),
            _ => erros_em_linha(&r.corpo, "", " | "),
        };
        return Err(ApiError::bad_request(format!("Focus NFe ({}): {msg}", r.http_status)));
    }
    Ok(Json(json!({
        "ok": true,
        "mensagem": format!("✅ Certificado enviado com sucesso para a Focus NFe ({})!", nome_ambiente(cfg.producao())),
        "arquivo": nome,
        "tamanho": format!("{:.1} KB", bytes.len() as f64 / 1024.0),
    })))
}

// ============================================================
// Validação do token e diagnóstico
// ============================================================

pub(super) async fn validate_api(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    let cfg = st.config().await?;
    let Some(focus) = st.focus(&cfg) else {
        return Err(ApiError::with_body(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": "Token da API não configurado. Preencha o Token nas Configurações Fiscais." }),
        ));
    };
    let producao = cfg.producao();
    let ambiente = nome_ambiente(producao);
    let cnpj = so_digitos(cfg.cnpj.as_deref().unwrap_or(""));
    let sem_conexao = |e: FiscalError| {
        Json(json!({
            "ok": false,
            "error": "Não foi possível conectar à Focus NFe. Verifique sua conexão com a internet.",
            "detalhe": e.to_string(),
        }))
    };

    let mut empresa: Option<Value> = None;
    if !cnpj.is_empty() {
        match focus.requisitar(Method::GET, &format!("/v2/empresas/{cnpj}"), None).await {
            Ok(r) if r.http_status == 401 => {
                let (este, outro) = if producao { ("PRODUÇÃO", "Homologação") } else { ("HOMOLOGAÇÃO", "Produção") };
                return Ok(Json(json!({
                    "ok": false,
                    "error": format!("Token não reconhecido pela Focus NFe (401 Não Autorizado).\n\nVerifique se o token é do ambiente de {este}. Se o token foi gerado no portal de {outro}, ajuste o campo \"Ambiente SEFAZ\" para coincidir."),
                    "status": 401,
                    "ambiente": ambiente,
                })));
            }
            Ok(r) if r.ok() && r.corpo.is_object() => empresa = Some(r.corpo),
            Ok(_) => {}
            Err(e) => tracing::warn!("validate-api: consulta direta do CNPJ falhou: {e}"),
        }
    }
    let lista = match focus.requisitar(Method::GET, "/v2/empresas", None).await {
        Ok(r) => r,
        Err(e) => return Ok(sem_conexao(e)),
    };
    if lista.http_status == 401 {
        return Ok(Json(json!({
            "ok": false,
            "error": "Token inválido ou sem permissão. Verifique se o Token está correto e é de produção/homologação conforme o ambiente configurado.",
            "status": 401,
            "ambiente": ambiente,
        })));
    }
    let mut total = 0;
    if lista.ok() {
        let empresas = lista.corpo.as_array().or_else(|| lista.corpo.get("empresas").and_then(Value::as_array)).cloned().unwrap_or_default();
        total = empresas.len();
        if empresa.is_none() && !cnpj.is_empty() {
            empresa = empresas.into_iter().find(|e| {
                so_digitos(&[txt(e, "cnpj"), txt(e, "cpf_cnpj"), txt(e, "document")].into_iter().find(|s| !s.is_empty()).unwrap_or_default())
                    == cnpj
            });
        }
    }
    let cnpj_cfg = cfg.cnpj.clone().filter(|c| !c.is_empty());
    let localizado = empresa.is_some();
    let descricao = match (&empresa, cnpj.is_empty()) {
        (Some(e), _) => format!(
            "✓ CNPJ encontrado na Focus NFe: {}",
            opt(e, "nome_fantasia").or_else(|| opt(e, "nome")).or(cnpj_cfg.clone()).unwrap_or_default()
        ),
        (None, false) => format!(
            "⚠️ CNPJ {} ainda não localizado no ambiente {ambiente} da Focus NFe. Preencha os dados da empresa e clique em \"Cadastrar Dados da Empresa\".",
            cnpj_cfg.clone().unwrap_or_default()
        ),
        (None, true) => "ℹ️ Nenhum CNPJ configurado ainda.".into(),
    };
    Ok(Json(json!({
        "ok": true,
        "ambiente": if producao { "🟢 Produção (Notas Oficiais)" } else { "🟡 Homologação (Testes)" },
        "totalEmpresas": if total > 0 { total } else { usize::from(localizado) },
        "cnpjConfigurado": cnpj_cfg,
        "cnpjLocalizado": localizado,
        "empresaCadastrada": descricao,
        "mensagem": if localizado {
            format!("Empresa localizada com sucesso na Focus NFe ({ambiente})!")
        } else {
            "Conexão com a Focus NFe estabelecida com sucesso. Cadastre sua empresa para emitir NFC-e.".to_string()
        },
    })))
}

pub(super) async fn diagnose_focus(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token não configurado.")?;
    let cnpj = so_digitos(cfg.cnpj.as_deref().unwrap_or(""));
    let lista = focus.requisitar(Method::GET, "/v2/empresas", None).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let (direta_status, direta) = if cnpj.is_empty() {
        (0, Value::Null)
    } else {
        let r = focus
            .requisitar(Method::GET, &format!("/v2/empresas/{cnpj}"), None)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        (r.http_status, r.corpo)
    };
    Ok(Json(json!({
        "ambiente": if cfg.producao() { "PRODUÇÃO" } else { "HOMOLOGAÇÃO" },
        "cnpjConfigurado": Some(cnpj).filter(|c| !c.is_empty()),
        "listaEmpresasStatus": lista.http_status,
        "listaEmpresasResposta": lista.corpo,
        "buscaDiretaStatus": direta_status,
        "buscaDiretaResposta": direta,
    })))
}

// ============================================================
// SEFAZ, NCM, backups, CNPJ
// ============================================================

#[derive(Deserialize)]
pub(super) struct UfQuery {
    uf: Option<String>,
}

pub(super) async fn sefaz_status(State(st): State<FiscalState>, Query(q): Query<UfQuery>) -> ApiResult<Json<Value>> {
    let cfg = st.config().await?;
    let uf = q
        .uf
        .or(cfg.uf.clone())
        .map(|u| u.trim().to_uppercase())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "PA".into());
    let autorizador = if uf == "PA" { "SEFAZ PA (SVRS)".to_string() } else { format!("SEFAZ {uf}") };

    // 1. Consulta oficial pela Focus
    let mut detalhe = String::new();
    match st.focus(&cfg) {
        Some(focus) => match focus.status_sefaz(&uf).await {
            Ok(s) => {
                return Ok(Json(json!({
                    "ok": true,
                    "online": s.online,
                    "status": s.status,
                    "codigoStatus": s.codigo_status,
                    "motivoStatus": s.motivo_status,
                    "tempoMedio": s.tempo_medio,
                    "uf": uf,
                    "autorizador": autorizador,
                })))
            }
            Err(FiscalError::Focus { mensagem, .. }) => detalhe = mensagem,
            Err(e) => tracing::warn!("status da SEFAZ pela Focus falhou: {e}"),
        },
        None => detalhe = "Token fiscal não configurado neste computador".into(),
    }

    // 2. Ping direto nos servidores da SEFAZ
    let latencia = publico::ping(&st.ping_sefaz).await;
    let online = latencia.is_some();
    let motivo = match (online, detalhe.is_empty()) {
        (true, true) => "Serviço em Operação".to_string(),
        (true, false) => format!("SEFAZ Ativa ({detalhe})"),
        (false, true) => "Servidores SEFAZ sem resposta".to_string(),
        (false, false) => format!("{detalhe} e servidores SEFAZ sem resposta"),
    };
    let mut r = json!({
        "ok": true,
        "online": online,
        "status": if online { "online" } else { "offline" },
        "codigoStatus": if online { "107" } else { "999" },
        "motivoStatus": motivo,
        "tempoMedio": latencia.unwrap_or(0.18),
        "uf": uf,
        "autorizador": autorizador,
    });
    if !detalhe.is_empty() {
        r["detalheToken"] = json!(detalhe);
    }
    Ok(Json(r))
}

pub(super) async fn ncm(State(st): State<FiscalState>, Path(codigo): Path<String>) -> ApiResult<Json<Value>> {
    let codigo = so_digitos(&codigo);
    if codigo.len() < 4 {
        return Err(ApiError::bad_request("Código NCM deve ter pelo menos 4 dígitos."));
    }
    let cfg = st.config().await?;
    let Some(focus) = st.focus(&cfg) else {
        return Ok(Json(json!({
            "ok": true,
            "codigo": codigo,
            "descricao": "Código NCM salvo localmente (configure o token da Focus para validação oficial)",
            "valido": true,
        })));
    };
    let r = focus.requisitar(Method::GET, &format!("/v2/ncms/{codigo}"), None).await.map_err(erro_fiscal)?;
    if r.http_status == 404 {
        return Err(ApiError::with_body(
            StatusCode::NOT_FOUND,
            json!({ "valido": false, "error": "Código NCM não encontrado na Receita Federal / Mercosul." }),
        ));
    }
    if !r.ok() {
        return Err(ApiError::with_body(
            StatusCode::from_u16(r.http_status).unwrap_or(StatusCode::BAD_GATEWAY),
            json!({ "valido": false, "error": opt(&r.corpo, "mensagem").unwrap_or_else(|| "Erro ao consultar NCM.".into()) }),
        ));
    }
    let d = &r.corpo;
    Ok(Json(json!({
        "ok": true,
        "valido": true,
        "codigo": opt(d, "codigo").unwrap_or(codigo),
        "descricao": txt(d, "descricao"),
        "dataInicio": d.get("data_inicio"),
        "dataFim": d.get("data_fim"),
        "aliquotaNacional": d.get("aliquota_nacional"),
        "aliquotaEstadual": d.get("aliquota_estadual"),
        "aliquotaImportado": d.get("aliquota_importado"),
    })))
}

pub(super) async fn focus_backups(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da Focus NFe não configurado.")?;
    let cnpj = so_digitos(cfg.cnpj.as_deref().unwrap_or(""));
    if cnpj.len() != 14 {
        return Err(ApiError::bad_request("CNPJ da empresa não configurado ou inválido."));
    }
    let r = focus.requisitar(Method::GET, &format!("/v2/backups/{cnpj}.json"), None).await.map_err(erro_fiscal)?;
    if r.http_status == 404 {
        return Ok(Json(json!({ "ok": true, "backups": [], "mensagem": "Nenhum backup em nuvem disponível para este CNPJ ainda." })));
    }
    if !r.ok() {
        return Err(ApiError::new(
            StatusCode::from_u16(r.http_status).unwrap_or(StatusCode::BAD_GATEWAY),
            opt(&r.corpo, "mensagem").unwrap_or_else(|| "Erro ao consultar backups na Focus NFe.".into()),
        ));
    }
    let backups = r.corpo.as_array().cloned().or_else(|| r.corpo.get("backups").and_then(Value::as_array).cloned()).unwrap_or_default();
    Ok(Json(json!({ "ok": true, "backups": backups })))
}

pub(super) async fn consulta_cnpj(State(st): State<FiscalState>, Path(cnpj): Path<String>) -> ApiResult<Json<publico::DadosCnpj>> {
    match publico::consultar_cnpj(&st.brasil_api_url, &cnpj).await {
        Ok(d) => Ok(Json(d)),
        Err(FiscalError::Focus { http_status, mensagem, .. }) => {
            Err(ApiError::new(StatusCode::from_u16(http_status).unwrap_or(StatusCode::NOT_FOUND), mensagem))
        }
        Err(FiscalError::Validacao(m)) => Err(ApiError::bad_request(m)),
        Err(e) => Err(ApiError::internal(format!("Erro ao consultar CNPJ: {e}"))),
    }
}
