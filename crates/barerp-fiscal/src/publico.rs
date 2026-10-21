//! Consultas fora da Focus: dados públicos do CNPJ (BrasilAPI) e ping dos servidores da SEFAZ.
//! Tradução de `/consulta-cnpj/:cnpj` e do segundo passo de `/sefaz-status` em `fiscal.ts`.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::error::FiscalError;
use crate::num::digits;

pub const BRASIL_API: &str = "https://brasilapi.com.br";

/// Servidores da SEFAZ do Pará (SVRS) e a própria Focus, na ordem do Node.
pub const PING_SEFAZ: [&str; 3] =
    ["https://dfe-portal.svrs.rs.gov.br/Nfe/Disponibilidade", "https://app.sefa.pa.gov.br", "https://api.focusnfe.com.br"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DadosCnpj {
    pub ok: bool,
    pub cnpj: String,
    pub razao_social: String,
    pub nome_fantasia: String,
    pub ie: String,
    pub cep: String,
    pub logradouro: String,
    pub numero: String,
    pub complemento: String,
    pub bairro: String,
    pub municipio: String,
    pub uf: String,
    pub telefone: String,
    pub cnae: Value,
}

fn txt(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Cadastro público do CNPJ na Receita (BrasilAPI). `base` é [`BRASIL_API`] fora dos testes.
pub async fn consultar_cnpj(base: &str, cnpj: &str) -> Result<DadosCnpj, FiscalError> {
    let cnpj = digits(cnpj);
    if cnpj.len() != 14 {
        return Err(FiscalError::Validacao("CNPJ deve conter 14 dígitos.".into()));
    }
    let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
    let res = http.get(format!("{}/api/cnpj/v1/{cnpj}", base.trim_end_matches('/'))).send().await?;
    if !res.status().is_success() {
        return Err(FiscalError::Focus {
            http_status: res.status().as_u16(),
            mensagem: "CNPJ não localizado na base pública da Receita Federal.".into(),
            detalhes: Value::Null,
        });
    }
    let d: Value = res.json().await?;
    let logradouro = [txt(&d, "descricao_tipo_de_logradouro"), txt(&d, "logradouro")]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let telefone = Some(txt(&d, "ddd_telefone_1")).filter(|t| !t.is_empty()).unwrap_or_else(|| txt(&d, "ddd_telefone_2"));
    Ok(DadosCnpj {
        ok: true,
        cnpj,
        razao_social: txt(&d, "razao_social"),
        nome_fantasia: txt(&d, "nome_fantasia"),
        ie: String::new(),
        cep: txt(&d, "cep"),
        logradouro,
        numero: txt(&d, "numero"),
        complemento: txt(&d, "complemento"),
        bairro: txt(&d, "bairro"),
        municipio: txt(&d, "municipio"),
        uf: txt(&d, "uf"),
        telefone,
        cnae: d.get("cnae_fiscal").cloned().unwrap_or(Value::String(String::new())),
    })
}

/// Tempo (s, duas casas) até o primeiro servidor que responder com status abaixo de 500, ou `None` se
/// nenhum responder em 6 s cada.
pub async fn ping(urls: &[String]) -> Option<f64> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(6)).build().ok()?;
    let inicio = Instant::now();
    for url in urls {
        if let Ok(res) = http.get(url).send().await {
            if res.status().as_u16() < 500 {
                return Some((inicio.elapsed().as_secs_f64() * 100.0).round() / 100.0);
            }
        }
    }
    None
}

pub const VIA_CEP: &str = "https://viacep.com.br";

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EnderecoCep {
    pub logradouro: String,
    pub bairro: String,
    pub municipio: String,
    pub uf: String,
}

/// Endereço do CEP pela BrasilAPI e, se ela não achar, pela ViaCEP. `None` se nenhuma achar.
pub async fn consultar_cep(brasil_api: &str, via_cep: &str, cep: &str) -> Option<EnderecoCep> {
    let cep = digits(cep);
    if cep.len() != 8 {
        return None;
    }
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().ok()?;
    if let Ok(r) = http.get(format!("{}/api/cep/v1/{cep}", brasil_api.trim_end_matches('/'))).send().await {
        if r.status().is_success() {
            if let Ok(d) = r.json::<Value>().await {
                if !txt(&d, "city").is_empty() {
                    return Some(EnderecoCep {
                        logradouro: txt(&d, "street"),
                        bairro: txt(&d, "neighborhood"),
                        municipio: txt(&d, "city"),
                        uf: txt(&d, "state"),
                    });
                }
            }
        }
    }
    let d: Value = http.get(format!("{}/ws/{cep}/json/", via_cep.trim_end_matches('/'))).send().await.ok()?.json().await.ok()?;
    if d.get("erro").is_some() {
        return None;
    }
    Some(EnderecoCep {
        logradouro: txt(&d, "logradouro"),
        bairro: txt(&d, "bairro"),
        municipio: txt(&d, "localidade"),
        uf: txt(&d, "uf"),
    })
}
