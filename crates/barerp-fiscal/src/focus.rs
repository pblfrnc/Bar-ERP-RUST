//! Cliente HTTP da Focus NFe (API v2). Só fala com a Focus: gravar `NotaEmitida`/`NotaRecebida`,
//! arquivar XML no cofre e montar a URL do DANFE do servidor ficam com quem chama.
//!
//! Status de emissão: só `autorizado` é sucesso; `processando*` é um estado próprio ([`Emissao::Processando`]);
//! qualquer outro (`erro_autorizacao`, `denegado`, `cancelado`...) vira [`FiscalError::Sefaz`].

use std::time::{Duration, Instant};

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::FiscalError;
use crate::num::{digits, parse_float};
use crate::payload::{FocusPayload, Modelo};
use crate::xml::parece_xml_nfe;

const PRODUCAO: &str = "https://api.focusnfe.com.br";
const HOMOLOGACAO: &str = "https://homologacao.focusnfe.com.br";

/// A Focus devolve no máximo 100 notas recebidas por consulta.
const PAGINA_RECEBIDAS: usize = 100;
const MAX_PAGINAS_RECEBIDAS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ambiente {
    Producao,
    Homologacao,
}

impl Ambiente {
    /// Valor da coluna `FiscalSettings.environment`: só "producao" é produção.
    pub fn from_setting(environment: &str) -> Self {
        if environment == "producao" {
            Ambiente::Producao
        } else {
            Ambiente::Homologacao
        }
    }

    pub fn base_url(self) -> &'static str {
        match self {
            Ambiente::Producao => PRODUCAO,
            Ambiente::Homologacao => HOMOLOGACAO,
        }
    }

    fn nome(self) -> &'static str {
        match self {
            Ambiente::Producao => "Produção",
            Ambiente::Homologacao => "Homologação",
        }
    }
}

/// Resposta crua da Focus: status HTTP e corpo JSON (texto vira `Value::String`; corpo vazio, `{}`).
#[derive(Debug, Clone, PartialEq)]
pub struct RespostaFocus {
    pub http_status: u16,
    pub corpo: Value,
}

impl RespostaFocus {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.http_status)
    }

    pub fn status(&self) -> &str {
        self.corpo.get("status").and_then(Value::as_str).unwrap_or("")
    }

    /// Caminho do DANFE (HTML da NFC-e ou PDF da NF-e) como a Focus devolve, relativo ou absoluto.
    pub fn caminho_danfe(&self) -> Option<String> {
        ["caminho_danfe", "danfe_url", "url_danfe"].iter().find_map(|k| texto(&self.corpo, k))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotaAutorizada {
    pub referencia: String,
    pub chave: Option<String>,
    pub numero: Option<String>,
    pub serie: Option<String>,
    pub caminho_xml: Option<String>,
    pub caminho_danfe: Option<String>,
    /// Corpo completo da Focus.
    pub dados: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Emissao {
    Autorizada(NotaAutorizada),
    /// A nota está na fila da SEFAZ; consultar depois pela referência.
    Processando { referencia: String, dados: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusSefaz {
    pub online: bool,
    pub status: String,
    pub codigo_status: String,
    pub motivo_status: String,
    pub tempo_medio: f64,
}

/// Uma nota de `/v2/nfes_recebidas`.
#[derive(Debug, Clone, PartialEq)]
pub struct NotaRecebidaFocus {
    /// 44 dígitos.
    pub chave: String,
    pub nome_emitente: Option<String>,
    /// Só dígitos.
    pub documento_emitente: Option<String>,
    pub numero: Option<String>,
    pub serie: Option<String>,
    /// 0 se ausente.
    pub valor_total: f64,
    pub data_emissao: Option<String>,
    pub manifestacao_destinatario: Option<String>,
    pub dados: Value,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListaRecebidas {
    pub notas: Vec<NotaRecebidaFocus>,
    /// Preenchido quando a Focus recusou uma página: `notas` tem o que veio antes.
    pub aviso: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct XmlRecebida {
    /// XML completo, quando a SEFAZ já liberou.
    pub xml: Option<String>,
    /// JSON da nota, quando veio pela consulta `.json`.
    pub dados: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arquivo {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Texto de um campo que a Focus manda ora como string, ora como número.
fn texto(v: &Value, campo: &str) -> Option<String> {
    match v.get(campo)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn numero(v: &Value, campo: &str) -> Option<f64> {
    match v.get(campo)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => parse_float(s),
        _ => None,
    }
}

/// Erros da Focus em uma linha por erro (`[campo] mensagem`), ou a mensagem geral.
fn formatar_erros(d: &Value) -> String {
    if let Some(erros) = d.get("erros").and_then(Value::as_array) {
        return erros
            .iter()
            .map(|e| {
                let campo = e.get("campo").and_then(Value::as_str).map(|c| format!("[{c}] ")).unwrap_or_default();
                let msg = texto(e, "mensagem").or_else(|| texto(e, "codigo")).unwrap_or_else(|| e.to_string());
                format!("{campo}{msg}")
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    texto(d, "mensagem_sefaz").or_else(|| texto(d, "mensagem")).unwrap_or_else(|| d.to_string())
}

fn nota_autorizada(d: &Value, referencia: &str) -> NotaAutorizada {
    NotaAutorizada {
        referencia: texto(d, "ref").unwrap_or_else(|| referencia.to_string()),
        chave: texto(d, "chave_nfe").or_else(|| texto(d, "chave")),
        numero: texto(d, "numero"),
        serie: texto(d, "serie"),
        caminho_xml: texto(d, "caminho_xml_nota_fiscal"),
        caminho_danfe: ["caminho_danfe", "danfe_url", "url_danfe"].iter().find_map(|k| texto(d, k)),
        dados: d.clone(),
    }
}

/// Interpreta o corpo de uma emissão ou consulta.
fn interpretar(modelo: Modelo, d: &Value, referencia: &str) -> Result<Emissao, FiscalError> {
    let status = d.get("status").and_then(Value::as_str).unwrap_or("");
    let nota = match modelo {
        Modelo::Nfce => "a NFC-e",
        Modelo::Nfe => "a NF-e",
    };
    let sefaz = |mensagem: String| FiscalError::Sefaz { status: status.to_string(), mensagem, detalhes: d.clone() };
    match status {
        "autorizado" => Ok(Emissao::Autorizada(nota_autorizada(d, referencia))),
        s if s.starts_with("processando") => Ok(Emissao::Processando {
            referencia: texto(d, "ref").unwrap_or_else(|| referencia.to_string()),
            dados: d.clone(),
        }),
        "erro_autorizacao" => Err(sefaz(format!("SEFAZ recusou {nota}: {}", formatar_erros(d)))),
        "cancelado" => Err(sefaz(format!(
            "{} desta venda foi cancelada. Emita uma nova pelo emissor, se necessário.",
            capitalizar(nota)
        ))),
        "denegado" => Err(sefaz(format!(
            "A SEFAZ denegou {nota} desta venda (situação fiscal do emitente ou do destinatário). \
             Não é possível reenviar; fale com a contabilidade."
        ))),
        outro => {
            let msg = if d.get("erros").is_some() || d.get("mensagem_sefaz").is_some() || d.get("mensagem").is_some() {
                formatar_erros(d)
            } else {
                format!("Status: {outro}")
            };
            Err(sefaz(format!("SEFAZ não autorizou {nota}: {msg}")))
        }
    }
}

fn capitalizar(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|p| p.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn validar_justificativa(texto: &str, o_que: &str) -> Result<String, FiscalError> {
    let t = texto.trim();
    if t.chars().count() < 15 {
        return Err(FiscalError::Validacao(format!(
            "A justificativa {o_que}deve ter no mínimo 15 caracteres (Regra da SEFAZ)."
        )));
    }
    Ok(t.to_string())
}

#[derive(Clone)]
pub struct FocusClient {
    http: reqwest::Client,
    base: String,
    token: String,
    cnpj: String,
    ambiente: Ambiente,
    espera_nfce: Duration,
    intervalo_nfe: Duration,
    limite_nfe: Duration,
}

impl FocusClient {
    /// `token` é o token da empresa na Focus (o `apiToken` já decifrado); `cnpj` é o do emitente.
    pub fn new(ambiente: Ambiente, token: &str, cnpj: &str) -> Result<Self, FiscalError> {
        Self::com_base_url(ambiente.base_url(), ambiente, token, cnpj)
    }

    /// Cliente apontando para outro endereço (servidor simulado nos testes).
    pub fn com_base_url(base_url: &str, ambiente: Ambiente, token: &str, cnpj: &str) -> Result<Self, FiscalError> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            token: token.trim().to_string(),
            cnpj: digits(cnpj),
            ambiente,
            espera_nfce: Duration::from_millis(2500),
            intervalo_nfe: Duration::from_secs(2),
            limite_nfe: Duration::from_secs(16),
        })
    }

    /// Esperas da emissão: NFC-e em processamento é consultada uma vez depois de `espera_nfce`;
    /// NF-e é consultada a cada `intervalo_nfe` até `limite_nfe`.
    pub fn com_esperas(mut self, espera_nfce: Duration, intervalo_nfe: Duration, limite_nfe: Duration) -> Self {
        self.espera_nfce = espera_nfce;
        self.intervalo_nfe = intervalo_nfe;
        self.limite_nfe = limite_nfe;
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn ambiente(&self) -> Ambiente {
        self.ambiente
    }

    fn req(&self, method: Method, caminho: &str) -> RequestBuilder {
        self.http.request(method, format!("{}{}", self.base, caminho)).basic_auth(&self.token, Some(""))
    }

    async fn enviar(&self, rb: RequestBuilder) -> Result<RespostaFocus, FiscalError> {
        let res = rb.send().await?;
        let http_status = res.status().as_u16();
        let texto = res.text().await?;
        // corpo que não é JSON (página de erro, texto) fica como string; vazio vira {}
        let corpo = match serde_json::from_str(&texto) {
            Ok(v) => v,
            Err(_) if texto.trim().is_empty() => json!({}),
            Err(_) => Value::String(texto),
        };
        Ok(RespostaFocus { http_status, corpo })
    }

    /// URL de um arquivo da Focus (caminho relativo ou absoluto). O token só vai para a própria Focus.
    fn url_arquivo(&self, caminho: &str) -> (String, bool) {
        let url = if caminho.starts_with("http") {
            caminho.to_string()
        } else {
            format!("{}{}{}", self.base, if caminho.starts_with('/') { "" } else { "/" }, caminho)
        };
        let com_token = url.starts_with(&format!("{}/", self.base)) || url.contains("focusnfe.com.br");
        (url, com_token)
    }

    /// Consulta uma nota emitida pela referência (`GET /v2/{nfce|nfe}/{ref}`). `completa` pede o XML/DANFE.
    pub async fn consultar(&self, modelo: Modelo, referencia: &str, completa: bool) -> Result<RespostaFocus, FiscalError> {
        let mut query: Vec<(&str, &str)> = Vec::new();
        if modelo == Modelo::Nfce && !self.cnpj.is_empty() {
            query.push(("cnpj_emitente", &self.cnpj));
        }
        if completa {
            query.push(("completa", "1"));
        }
        let caminho = format!("/v2/{}/{}", modelo.caminho(), url_seg(referencia));
        self.enviar(self.req(Method::GET, &caminho).query(&query)).await
    }

    fn erro_envio(&self, modelo: Modelo, r: &RespostaFocus) -> FiscalError {
        let mut mensagem = formatar_erros(&r.corpo);
        let nao_autorizado = r.http_status == 403
            || r.corpo.get("codigo").and_then(Value::as_str) == Some("cnpj_nao_autorizado")
            || r.corpo.to_string().contains("não autorizado");
        if modelo == Modelo::Nfce && nao_autorizado {
            mensagem = format!(
                "CNPJ {} não autorizado pela Focus NFe para emissão de NFC-e.\n\n\
                 Principais motivos e como resolver:\n\
                 1. Token da Empresa: No modelo Software House, verifique se está usando o Token específico desta \
                 Empresa (encontrado em \"Empresas > [Sua Empresa] > Tokens\" no portal da Focus NFe) em vez do Token Master.\n\
                 2. CSC da SEFAZ: O Código CSC e o ID do Token devem estar cadastrados na empresa (gerados no portal da \
                 SEFAZ do seu estado).\n\
                 3. Credenciamento SEFAZ: Confirme com a contabilidade se o CNPJ já está credenciado na SEFAZ para \
                 emitir NFC-e (modelo 65) no ambiente {}.",
                self.cnpj,
                match self.ambiente {
                    Ambiente::Producao => "de Produção",
                    Ambiente::Homologacao => "de Homologação",
                }
            );
        }
        FiscalError::Focus {
            http_status: r.http_status,
            mensagem: format!("Focus NFe ({}): {mensagem}", r.http_status),
            detalhes: r.corpo.clone(),
        }
    }

    /// Emite a NFC-e. Com `referencia_fixa` (referência da venda, ver [`crate::referencia_nfce`]) consulta antes
    /// de enviar: a nota pode ter sido autorizada numa tentativa que deu erro do nosso lado. Se a SEFAZ ainda
    /// estiver processando, consulta mais uma vez depois da espera configurada.
    pub async fn emitir_nfce(
        &self,
        referencia: &str,
        payload: &FocusPayload,
        referencia_fixa: bool,
    ) -> Result<Emissao, FiscalError> {
        if referencia_fixa {
            // falha de rede na consulta prévia não impede o envio
            if let Ok(prev) = self.consultar(Modelo::Nfce, referencia, false).await {
                let s = prev.status();
                if prev.ok() && (matches!(s, "autorizado" | "cancelado" | "denegado") || s.starts_with("processando")) {
                    return interpretar(Modelo::Nfce, &prev.corpo, referencia);
                }
                // erro_autorizacao ou nota inexistente: reenvia com a mesma referência
            }
        }
        let query = [("ref", referencia), ("cnpj_emitente", self.cnpj.as_str()), ("dry_run", "0")];
        let r = self.enviar(self.req(Method::POST, "/v2/nfce").query(&query).json(payload)).await?;
        if !r.ok() {
            return Err(self.erro_envio(Modelo::Nfce, &r));
        }
        match interpretar(Modelo::Nfce, &r.corpo, referencia)? {
            Emissao::Processando { referencia: ref_focus, dados } => {
                tokio::time::sleep(self.espera_nfce).await;
                match self.consultar(Modelo::Nfce, &ref_focus, false).await {
                    Ok(check) if check.ok() => interpretar(Modelo::Nfce, &check.corpo, &ref_focus),
                    // a nota já foi aceita; sem a consulta, continua na fila
                    _ => Ok(Emissao::Processando { referencia: ref_focus, dados }),
                }
            }
            autorizada => Ok(autorizada),
        }
    }

    /// Emite a NF-e e, se a SEFAZ estiver processando, consulta (`completa=1`) até o limite configurado.
    pub async fn emitir_nfe(&self, referencia: &str, payload: &FocusPayload) -> Result<Emissao, FiscalError> {
        let query = [("ref", referencia), ("dry_run", "0")];
        let r = self.enviar(self.req(Method::POST, "/v2/nfe").query(&query).json(payload)).await?;
        if !r.ok() {
            return Err(self.erro_envio(Modelo::Nfe, &r));
        }
        let inicio = Instant::now();
        let mut atual = r.corpo;
        while atual.get("status").and_then(Value::as_str).is_some_and(|s| s.starts_with("processando"))
            && inicio.elapsed() < self.limite_nfe
        {
            tokio::time::sleep(self.intervalo_nfe).await;
            if let Ok(check) = self.consultar(Modelo::Nfe, referencia, true).await {
                if check.corpo.get("status").is_some() {
                    atual = check.corpo;
                }
            }
        }
        interpretar(Modelo::Nfe, &atual, referencia)
    }

    /// Cancela uma nota autorizada (`DELETE /v2/{nfce|nfe}/{ref}`, justificativa no corpo).
    pub async fn cancelar(&self, modelo: Modelo, referencia: &str, justificativa: &str) -> Result<Value, FiscalError> {
        let justificativa = validar_justificativa(justificativa, "")?;
        let caminho = format!("/v2/{}/{}", modelo.caminho(), url_seg(referencia));
        let r = self.enviar(self.req(Method::DELETE, &caminho).json(&json!({ "justificativa": justificativa }))).await?;
        let o_que = match modelo {
            Modelo::Nfce => "nota",
            Modelo::Nfe => "NF-e",
        };
        if !r.ok() {
            let msg = texto(&r.corpo, "mensagem").or_else(|| texto(&r.corpo, "codigo")).unwrap_or_else(|| r.corpo.to_string());
            return Err(FiscalError::Focus {
                http_status: r.http_status,
                mensagem: format!("Erro ao cancelar {o_que}: {msg}"),
                detalhes: r.corpo,
            });
        }
        if r.status() == "erro_cancelamento" {
            return Err(FiscalError::Sefaz {
                status: r.status().to_string(),
                mensagem: format!("Erro ao cancelar {o_que}: {}", formatar_erros(&r.corpo)),
                detalhes: r.corpo,
            });
        }
        Ok(r.corpo)
    }

    /// Carta de correção de NF-e (`POST /v2/nfe/{ref}/carta_correcao`).
    pub async fn carta_correcao(&self, referencia: &str, correcao: &str) -> Result<Value, FiscalError> {
        let correcao = correcao.trim();
        if correcao.chars().count() < 15 {
            return Err(FiscalError::validacao("O texto da Carta de Correção deve conter no mínimo 15 caracteres."));
        }
        let caminho = format!("/v2/nfe/{}/carta_correcao", url_seg(referencia));
        let r = self.enviar(self.req(Method::POST, &caminho).json(&json!({ "correcao": correcao }))).await?;
        if !r.ok() {
            let msg = texto(&r.corpo, "mensagem").or_else(|| texto(&r.corpo, "codigo")).unwrap_or_else(|| r.corpo.to_string());
            return Err(FiscalError::Focus {
                http_status: r.http_status,
                mensagem: format!("Erro na Carta de Correção: {msg}"),
                detalhes: r.corpo,
            });
        }
        Ok(r.corpo)
    }

    /// Inutiliza uma faixa de numeração (`POST /v2/{nfce|nfe}/inutilizacao`).
    pub async fn inutilizar(
        &self,
        modelo: Modelo,
        serie: &str,
        numero_inicial: u32,
        numero_final: u32,
        justificativa: &str,
    ) -> Result<Value, FiscalError> {
        if serie.trim().is_empty() || numero_inicial == 0 || numero_final == 0 {
            return Err(FiscalError::validacao("Série, número inicial e número final são obrigatórios."));
        }
        let justificativa = validar_justificativa(justificativa, "de inutilização ")?;
        if self.cnpj.is_empty() {
            return Err(FiscalError::validacao("CNPJ da empresa não configurado."));
        }
        let corpo = json!({
            "cnpj": self.cnpj,
            "serie": serie.trim(),
            "numero_inicial": numero_inicial,
            "numero_final": numero_final,
            "justificativa": justificativa,
        });
        let caminho = format!("/v2/{}/inutilizacao", modelo.caminho());
        let r = self.enviar(self.req(Method::POST, &caminho).json(&corpo)).await?;
        let padrao = match modelo {
            Modelo::Nfce => "Erro ao homologar inutilização de NFC-e na SEFAZ.",
            Modelo::Nfe => "Erro ao homologar inutilização na SEFAZ.",
        };
        if !r.ok() {
            let msg = texto(&r.corpo, "mensagem").or_else(|| texto(&r.corpo, "erro")).unwrap_or_else(|| padrao.into());
            return Err(FiscalError::Focus { http_status: r.http_status, mensagem: msg, detalhes: r.corpo });
        }
        if r.status() == "erro_autorizacao" {
            return Err(FiscalError::Sefaz {
                status: r.status().to_string(),
                mensagem: format!("{padrao} {}", formatar_erros(&r.corpo)),
                detalhes: r.corpo,
            });
        }
        Ok(r.corpo)
    }

    /// Status do serviço da SEFAZ da UF (`GET /v2/nfce/status`), com limite de 6 s.
    pub async fn status_sefaz(&self, uf: &str) -> Result<StatusSefaz, FiscalError> {
        let uf = uf.trim().to_uppercase();
        let rb = self.req(Method::GET, "/v2/nfce/status").query(&[("uf", uf.as_str())]).timeout(Duration::from_secs(6));
        let r = self.enviar(rb).await?;
        if r.http_status == StatusCode::UNAUTHORIZED.as_u16() {
            return Err(FiscalError::Focus {
                http_status: 401,
                mensagem: format!("Token Focus NFe não autorizado para {}", self.ambiente.nome()),
                detalhes: r.corpo,
            });
        }
        if !r.ok() {
            return Err(FiscalError::Focus {
                http_status: r.http_status,
                mensagem: format!("Focus NFe retornou status HTTP {}", r.http_status),
                detalhes: r.corpo,
            });
        }
        let d = &r.corpo;
        let status = r.status().to_lowercase();
        let codigo = texto(d, "codigo_status");
        let online = status == "online" || status == "ativo" || status == "ok" || codigo.as_deref() == Some("107");
        Ok(StatusSefaz {
            online,
            status: if status.is_empty() { if online { "online" } else { "offline" }.into() } else { status },
            codigo_status: codigo.unwrap_or_else(|| if online { "107" } else { "999" }.into()),
            motivo_status: texto(d, "motivo_status")
                .unwrap_or_else(|| if online { "Serviço em Operação" } else { "Serviço com Instabilidade" }.into()),
            tempo_medio: numero(d, "tempo_medio").filter(|t| *t > 0.0).unwrap_or(0.18),
        })
    }

    /// Todas as notas recebidas pelo CNPJ (`GET /v2/nfes_recebidas`), página a página pelo `X-Max-Version`
    /// (até 100 páginas de 100 notas). Sem a paginação, depois das 100 primeiras nenhuma nota nova entrava.
    pub async fn listar_nfes_recebidas(&self) -> Result<ListaRecebidas, FiscalError> {
        let mut lista = ListaRecebidas::default();
        let mut versao: u64 = 0;
        for _ in 0..MAX_PAGINAS_RECEBIDAS {
            let mut query: Vec<(&str, String)> = Vec::new();
            if !self.cnpj.is_empty() {
                query.push(("cnpj", self.cnpj.clone()));
            }
            if versao > 0 {
                query.push(("versao", versao.to_string()));
            }
            let res = self.req(Method::GET, "/v2/nfes_recebidas").query(&query).header("Accept", "application/json").send().await?;
            let http_status = res.status();
            let header_max = res
                .headers()
                .get("X-Max-Version")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(0);
            let corpo = res.text().await?;
            if !http_status.is_success() {
                let trecho: String = corpo.chars().take(300).collect();
                lista.aviso = Some(format!(
                    "Focus NFe respondeu {} ao listar notas recebidas (versao={versao}): {trecho}",
                    http_status.as_u16()
                ));
                break;
            }
            let pagina = match serde_json::from_str::<Value>(&corpo) {
                Ok(Value::Array(p)) if !p.is_empty() => p,
                _ => break,
            };
            let max_itens = pagina
                .iter()
                .filter_map(|n| numero(n, "versao"))
                .map(|v| v as u64)
                .max()
                .unwrap_or(0);
            let proxima = if header_max > 0 { header_max } else { max_itens };
            let tamanho = pagina.len();
            lista.notas.extend(pagina.into_iter().filter_map(nota_recebida));
            if tamanho < PAGINA_RECEBIDAS || proxima <= versao {
                break;
            }
            versao = proxima;
        }
        Ok(lista)
    }

    /// Manifesta ciência da operação de uma nota recebida.
    pub async fn manifestar_ciencia(&self, chave: &str) -> Result<Value, FiscalError> {
        let caminho = format!("/v2/nfes_recebidas/{}/manifesto", digits(chave));
        let r = self.enviar(self.req(Method::POST, &caminho).json(&json!({ "tipo": "ciencia" }))).await?;
        if !r.ok() {
            return Err(FiscalError::Focus {
                http_status: r.http_status,
                mensagem: format!("Focus NFe ({}): {}", r.http_status, formatar_erros(&r.corpo)),
                detalhes: r.corpo,
            });
        }
        Ok(r.corpo)
    }

    /// XML de uma nota recebida: tenta o `.xml` direto, o `.json` com `caminho_xml_nota_fiscal` e, se o `.json`
    /// falhar, o endpoint legado `/xml` e a rota sem extensão (`fetchNfeRecebidaXml`).
    /// `None` se a Focus não tiver nada da nota. Nunca falha: erro de rede conta como "ainda não disponível".
    pub async fn baixar_xml_recebida(&self, chave: &str) -> Option<XmlRecebida> {
        let chave = digits(chave);
        let xml = self
            .req(Method::GET, &format!("/v2/nfes_recebidas/{chave}.xml"))
            .header("Accept", "application/xml, text/xml, */*")
            .send()
            .await;
        if let Ok(res) = xml {
            if res.status().is_success() {
                if let Ok(t) = res.text().await {
                    if parece_xml_nfe(&t) {
                        return Some(XmlRecebida { xml: Some(t), dados: None });
                    }
                }
            }
        }
        let json = self
            .enviar(self.req(Method::GET, &format!("/v2/nfes_recebidas/{chave}.json")).header("Accept", "application/json"))
            .await
            .ok();
        if let Some(r) = json.filter(RespostaFocus::ok) {
            if let Some(caminho) = texto(&r.corpo, "caminho_xml_nota_fiscal") {
                if let Ok(arquivo) = self.baixar(&caminho).await {
                    let t = String::from_utf8_lossy(&arquivo.bytes).into_owned();
                    if parece_xml_nfe(&t) {
                        return Some(XmlRecebida { xml: Some(t), dados: Some(r.corpo) });
                    }
                }
            }
            return Some(XmlRecebida { xml: None, dados: Some(r.corpo) });
        }
        // sem o .json: endpoint legado /xml e a rota sem extensão
        for caminho in [format!("/v2/nfes_recebidas/{chave}/xml"), format!("/v2/nfes_recebidas/{chave}")] {
            let res = self
                .req(Method::GET, &caminho)
                .header("Accept", "application/xml, text/xml, application/json, */*")
                .send()
                .await;
            if let Ok(res) = res {
                if res.status().is_success() {
                    if let Ok(t) = res.text().await {
                        if parece_xml_nfe(&t) {
                            return Some(XmlRecebida { xml: Some(t), dados: None });
                        }
                    }
                }
            }
        }
        None
    }

    /// Chamada genérica à API da Focus (`/v2/empresas`, `/v2/ncms`, `/v2/backups`, e-mail...) com o token.
    /// Erro só em falha de rede; o status HTTP vem na resposta.
    pub async fn requisitar(&self, metodo: Method, caminho: &str, corpo: Option<&Value>) -> Result<RespostaFocus, FiscalError> {
        let mut rb = self.req(metodo, caminho);
        if let Some(c) = corpo {
            rb = rb.json(c);
        }
        self.enviar(rb).await
    }

    /// Baixa um arquivo da Focus (XML, DANFE) pelo caminho que ela devolveu, seguindo redirecionamentos.
    pub async fn baixar(&self, caminho: &str) -> Result<Arquivo, FiscalError> {
        let (url, com_token) = self.url_arquivo(caminho);
        let mut rb = self.http.get(&url);
        if com_token {
            rb = rb.basic_auth(&self.token, Some(""));
        }
        let res = rb.send().await?;
        let status = res.status();
        let content_type =
            res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let bytes = res.bytes().await?.to_vec();
        if !status.is_success() {
            return Err(FiscalError::Focus {
                http_status: status.as_u16(),
                mensagem: format!("Focus NFe ({}) ao baixar {url}", status.as_u16()),
                detalhes: Value::Null,
            });
        }
        Ok(Arquivo { content_type, bytes })
    }

    /// PDF do DANFE de uma NF-e direto pela chave, quando a consulta pela referência falha.
    pub async fn baixar_danfe_pdf_por_chave(&self, chave: &str) -> Option<Vec<u8>> {
        let chave = digits(chave);
        let k = crate::chave::parse_chave_acesso(&chave)?;
        let aamm = format!("20{}", &chave[2..6]);
        let caminhos = [
            format!("/arquivos/{}/{aamm}/DANFEs/{chave}.pdf", k.cnpj_emitente),
            format!("/v2/nfe/{chave}.pdf"),
            format!("/v2/nfes_recebidas/{chave}.pdf"),
        ];
        for caminho in &caminhos {
            // /arquivos é público na Focus: vai sem o token
            let mut rb = self.http.get(format!("{}{caminho}", self.base));
            if !caminho.starts_with("/arquivos") {
                rb = rb.basic_auth(&self.token, Some(""));
            }
            let Ok(res) = rb.send().await else { continue };
            let ct = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            if res.status().is_success() && (ct.contains("pdf") || ct.contains("octet-stream")) {
                if let Ok(b) = res.bytes().await {
                    return Some(b.to_vec());
                }
            }
        }
        None
    }
}

fn nota_recebida(n: Value) -> Option<NotaRecebidaFocus> {
    let chave = digits(&texto(&n, "chave_nfe").or_else(|| texto(&n, "chave"))?);
    if chave.is_empty() {
        return None;
    }
    Some(NotaRecebidaFocus {
        chave,
        nome_emitente: texto(&n, "nome_emitente"),
        documento_emitente: texto(&n, "documento_emitente")
            .or_else(|| texto(&n, "cnpj_emitente"))
            .map(|d| digits(&d))
            .filter(|d| !d.is_empty()),
        numero: texto(&n, "numero"),
        serie: texto(&n, "serie"),
        valor_total: numero(&n, "valor_total").unwrap_or(0.0),
        data_emissao: texto(&n, "data_emissao"),
        manifestacao_destinatario: texto(&n, "manifestacao_destinatario"),
        dados: n,
    })
}

/// Escapa a referência para um segmento de URL (`encodeURIComponent`).
pub(crate) fn url_seg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreta_status() {
        let ok = interpretar(Modelo::Nfce, &json!({"status":"autorizado","ref":"r1","chave_nfe":"k","numero":12,"serie":"1"}), "x");
        let Ok(Emissao::Autorizada(n)) = ok else { panic!() };
        assert_eq!((n.referencia.as_str(), n.numero.as_deref()), ("r1", Some("12")));

        let p = interpretar(Modelo::Nfe, &json!({"status":"processando_autorizacao"}), "x").unwrap();
        assert!(matches!(p, Emissao::Processando { referencia, .. } if referencia == "x"));

        let e = interpretar(Modelo::Nfce, &json!({"status":"erro_autorizacao","mensagem_sefaz":"Rejeição 778"}), "x").unwrap_err();
        assert_eq!(e.to_string(), "SEFAZ recusou a NFC-e: Rejeição 778");
        assert!(matches!(e, FiscalError::Sefaz { status, .. } if status == "erro_autorizacao"));

        let e = interpretar(Modelo::Nfce, &json!({"status":"denegado"}), "x").unwrap_err().to_string();
        assert!(e.starts_with("A SEFAZ denegou a NFC-e desta venda"));
        let e = interpretar(Modelo::Nfce, &json!({"status":"cancelado"}), "x").unwrap_err().to_string();
        assert!(e.starts_with("A NFC-e desta venda foi cancelada"));
        let e = interpretar(Modelo::Nfe, &json!({"status":"estranho"}), "x").unwrap_err().to_string();
        assert_eq!(e, "SEFAZ não autorizou a NF-e: Status: estranho");
        let e = interpretar(Modelo::Nfe, &json!({}), "x").unwrap_err().to_string();
        assert_eq!(e, "SEFAZ não autorizou a NF-e: Status: ");
    }

    #[test]
    fn erros_formatados() {
        let d = json!({"erros":[{"campo":"itens[0].ncm","mensagem":"inválido"},{"codigo":"x"}]});
        assert_eq!(formatar_erros(&d), "[itens[0].ncm] inválido\nx");
        assert_eq!(formatar_erros(&json!({"mensagem":"m"})), "m");
        assert_eq!(formatar_erros(&json!({"a":1})), "{\"a\":1}");
    }

    #[test]
    fn segmento_de_url() {
        assert_eq!(url_seg("nfce_ab-1.x"), "nfce_ab-1.x");
        assert_eq!(url_seg("a b/ç"), "a%20b%2F%C3%A7");
    }

    #[test]
    fn ambientes() {
        assert_eq!(Ambiente::from_setting("producao"), Ambiente::Producao);
        assert_eq!(Ambiente::from_setting(""), Ambiente::Homologacao);
        assert_eq!(Ambiente::Producao.base_url(), "https://api.focusnfe.com.br");
    }
}
