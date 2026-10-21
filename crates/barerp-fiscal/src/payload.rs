//! Montagem do JSON de emissão da Focus NFe para NFC-e (modelo 65) e NF-e (modelo 55).
//!
//! NFC-e: `router.post('/emit-nfce')` de `fiscal.ts` + rateio do desconto de `client/src/services/nfce.ts`.
//! NF-e: `buildNfePayload` de `nfeService.ts`.

use barerp_core::fiscal_doc::{cpf_cnpj_error, sanitize_cest, NFCE_LIMITE_SEM_IDENTIFICACAO};
use barerp_core::pricing::round2;
use chrono::{DateTime, FixedOffset, Utc};
use rand::Rng;
use serde::Serialize;

use crate::error::FiscalError;
use crate::num::{digits, fixed2, take_chars, to_fixed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modelo {
    /// Modelo 65, cupom do consumidor.
    Nfce,
    /// Modelo 55.
    Nfe,
}

impl Modelo {
    /// Trecho da URL da Focus (`/v2/nfce`, `/v2/nfe`).
    pub fn caminho(self) -> &'static str {
        match self {
            Modelo::Nfce => "nfce",
            Modelo::Nfe => "nfe",
        }
    }
}

/// Dados do emitente, da tabela `FiscalSettings`.
#[derive(Debug, Clone, Default)]
pub struct Emitente {
    pub cnpj: String,
    /// "3" = regime normal; qualquer outro valor é Simples Nacional.
    pub crt: String,
    pub serie_nfce: String,
    pub serie_nfe: String,
    pub cep: String,
    pub logradouro: String,
    pub numero: String,
    pub bairro: String,
    pub municipio: String,
    pub uf: String,
}

/// Um item da nota. Campos de texto vazios contam como não informados.
#[derive(Debug, Clone, Default)]
pub struct ItemNota {
    pub nome: String,
    /// 0 vira 1, como no Node.
    pub quantidade: f64,
    /// Valor unitário cobrado (já com promoção e adicionais).
    pub preco: f64,
    /// Desconto do item em reais (ver [`ratear_desconto`]). Limitado ao valor bruto do item.
    pub desconto: f64,
    pub ncm: String,
    /// Vazio vira 5102.
    pub cfop: String,
    /// Vazio vira UN.
    pub unidade: String,
    pub ean: String,
    /// Código interno do produto, usado quando não há EAN.
    pub codigo: String,
    pub cest: String,
}

/// Destinatário. Na NFC-e só o documento e o nome são usados.
#[derive(Debug, Clone, Default)]
pub struct Destinatario {
    /// CPF ou CNPJ, com ou sem pontuação. Vazio = consumidor não identificado.
    pub documento: String,
    pub nome: String,
    pub cep: String,
    pub logradouro: String,
    pub numero: String,
    pub complemento: String,
    pub bairro: String,
    pub municipio: String,
    pub uf: String,
}

#[derive(Debug, Clone, Default)]
pub struct Venda {
    pub itens: Vec<ItemNota>,
    pub destinatario: Destinatario,
    /// Forma de pagamento do caixa (PIX, CREDIT_CARD, DEBIT_CARD, CASH, CREDIT_TAB...).
    pub forma_pagamento: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FocusItem {
    pub numero_item: String,
    pub codigo_produto: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codigo_barras: Option<String>,
    pub descricao: String,
    pub cfop: String,
    pub codigo_ncm: String,
    pub ncm: String,
    pub unidade_comercial: String,
    pub quantidade_comercial: String,
    pub valor_unitario_comercial: String,
    pub valor_bruto: String,
    pub unidade_tributavel: String,
    pub quantidade_tributavel: String,
    pub valor_unitario_tributavel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valor_desconto: Option<String>,
    pub inclui_no_total: String,
    pub icms_origem: String,
    pub icms_situacao_tributaria: String,
    pub pis_situacao_tributaria: String,
    pub cofins_situacao_tributaria: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codigo_cest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FormaPagamento {
    pub forma_pagamento: String,
    pub valor_pagamento: String,
}

/// Endereço do destinatário, exigido na NF-e.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnderecoDestinatario {
    pub logradouro_destinatario: String,
    pub numero_destinatario: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complemento_destinatario: Option<String>,
    pub bairro_destinatario: String,
    pub municipio_destinatario: String,
    pub uf_destinatario: String,
    pub cep_destinatario: String,
    pub indicador_inscricao_estadual_destinatario: String,
}

/// Corpo do POST de emissão.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FocusPayload {
    pub cnpj_emitente: String,
    pub data_emissao: String,
    pub natureza_operacao: String,
    pub tipo_documento: String,
    pub finalidade_emissao: String,
    pub consumidor_final: String,
    pub presenca_comprador: String,
    pub modalidade_frete: String,
    pub local_destino: String,
    pub serie: String,
    /// Só na NF-e (a NFC-e manda a referência apenas na URL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referencia: Option<String>,
    pub itens: Vec<FocusItem>,
    pub formas_pagamento: Vec<FormaPagamento>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpf_destinatario: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cnpj_destinatario: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nome_destinatario: Option<String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub endereco: Option<EnderecoDestinatario>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotaMontada {
    pub payload: FocusPayload,
    /// Valor líquido da nota (o que vai em `valor_pagamento` e na `NotaEmitida.valorTotal`).
    pub valor_total: f64,
}

/// Código SEFAZ da forma de pagamento (tPag).
pub fn codigo_forma_pagamento(forma: &str) -> &'static str {
    match forma.trim().to_uppercase().as_str() {
        "PIX" => "17",
        "CREDITO" | "CARTÃO DE CRÉDITO" | "CREDIT_CARD" | "CARD" => "03",
        "DEBITO" | "CARTÃO DE DÉBITO" | "DEBIT_CARD" => "04",
        "CREDIT_TAB" => "05", // crédito loja (fiado)
        _ => "01",            // dinheiro
    }
}

/// Rateio proporcional do desconto da venda pelos itens (`emitNfceForOrder`), para o total da nota bater
/// com o valor pago. `totais_itens` é o total cobrado de cada item. Cada parte arredonda para baixo e nunca
/// passa do valor do item; a sobra de centavos vai, de trás para frente, para quem ainda tem margem.
pub fn ratear_desconto(totais_itens: &[f64], desconto: f64) -> Vec<f64> {
    let bruto: f64 = totais_itens.iter().sum();
    let desconto = round2(desconto).min(round2(bruto));
    let brutos: Vec<f64> = totais_itens.iter().map(|&t| round2(t)).collect();
    let floor2 = |v: f64| round2(v * 100.0).floor() / 100.0;
    let mut partes: Vec<f64> = brutos
        .iter()
        .map(|&g| if desconto > 0.0 && bruto > 0.0 { g.min(floor2(desconto * g / bruto)) } else { 0.0 })
        .collect();
    let mut sobra = round2(desconto - partes.iter().sum::<f64>());
    for i in (0..partes.len()).rev() {
        if sobra <= 0.0 {
            break;
        }
        let add = sobra.min(round2(brutos[i] - partes[i]));
        partes[i] = round2(partes[i] + add);
        sobra = round2(sobra - add);
    }
    partes
}

fn limpar_ref(s: &str, max: usize) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(max).collect()
}

fn aleatorio() -> u32 {
    rand::thread_rng().gen_range(1000..10000)
}

fn referencia_nfce_com(sale_key: Option<&str>, order_id: Option<&str>, agora_ms: i64, rnd: u32) -> String {
    let sale_key = sale_key.map(|k| limpar_ref(k, 40)).unwrap_or_default();
    if !sale_key.is_empty() {
        return format!("nfce_{sale_key}");
    }
    match order_id.filter(|o| !o.is_empty()) {
        Some(o) => format!("nfce_{}_{rnd}", limpar_ref(o, 20)),
        None => format!("nfce_{agora_ms}_{rnd}"),
    }
}

/// Referência da NFC-e na Focus. Com `sale_key` (id da venda) é fixa: tentar de novo nunca gera uma segunda
/// nota da mesma venda. Sem ela, `nfce_<orderId>_<aleatório>` ou `nfce_<timestamp>_<aleatório>`.
pub fn referencia_nfce(sale_key: Option<&str>, order_id: Option<&str>) -> String {
    referencia_nfce_com(sale_key, order_id, Utc::now().timestamp_millis(), aleatorio())
}

fn referencia_nfe_com(order_id: Option<&str>, agora_ms: i64, rnd: u32) -> String {
    match order_id.filter(|o| !o.is_empty()) {
        Some(o) => format!("nfe_{}_{rnd}", limpar_ref(o, 20)),
        None => format!("nfe_{agora_ms}_{rnd}"),
    }
}

/// Referência da NF-e na Focus: `nfe_<orderId>_<aleatório>` ou `nfe_<timestamp>_<aleatório>`.
pub fn referencia_nfe(order_id: Option<&str>) -> String {
    referencia_nfe_com(order_id, Utc::now().timestamp_millis(), aleatorio())
}

/// Data de emissão ISO 8601 no horário de Brasília (`-03:00`; o Brasil não tem horário de verão desde 2019).
pub fn data_emissao(agora: DateTime<Utc>) -> String {
    let brt = FixedOffset::west_opt(3 * 3600).expect("offset válido");
    agora.with_timezone(&brt).format("%Y-%m-%dT%H:%M:%S-03:00").to_string()
}

fn ou<'a>(valor: &'a str, padrao: &'a str) -> &'a str {
    if valor.is_empty() {
        padrao
    } else {
        valor
    }
}

/// Valor bruto do item com o mesmo arredondamento que vai na nota (`Number((qty*price).toFixed(2))`).
fn bruto_item(i: &ItemNota) -> f64 {
    fixed2(quantidade(i) * i.preco)
}

fn quantidade(i: &ItemNota) -> f64 {
    if i.quantidade == 0.0 || i.quantidade.is_nan() {
        1.0
    } else {
        i.quantidade
    }
}

fn desconto_item(i: &ItemNota) -> f64 {
    round2(bruto_item(i).min(i.desconto.max(0.0)))
}

fn validar_itens(venda: &Venda) -> Result<(), FiscalError> {
    if venda.itens.is_empty() {
        return Err(FiscalError::validacao("Nenhum item adicionado para a nota."));
    }
    // Documento informado deve estar completo e válido (nunca descartar em silêncio)
    if let Some(erro) = cpf_cnpj_error(&venda.destinatario.documento) {
        return Err(FiscalError::validacao(erro));
    }
    // NCM de 8 dígitos é obrigatório: nunca usar um NCM genérico que não corresponde ao produto
    if let Some(item) = venda.itens.iter().find(|i| digits(&i.ncm).len() != 8) {
        return Err(FiscalError::Validacao(format!(
            "O produto \"{}\" está sem NCM válido (8 dígitos). Cadastre o NCM antes de emitir.",
            ou(&item.nome, "sem nome")
        )));
    }
    Ok(())
}

fn montar_item(emitente: &Emitente, i: &ItemNota, index: usize) -> FocusItem {
    let ncm = digits(&i.ncm);
    let cfop = digits(&i.cfop);
    let cfop = if cfop.is_empty() { "5102".to_string() } else { cfop };
    let qty = quantidade(i);
    let unidade = take_chars(&ou(&i.unidade, "UN").to_uppercase(), 6);
    // codigo_produto: EAN > código interno > número sequencial (nunca o id do banco)
    let ean = digits(&i.ean);
    let ean = (ean.len() >= 8).then_some(ean);
    let codigo = i.codigo.trim();
    let numero_item = (index + 1).to_string();
    let codigo_produto = take_chars(ean.as_deref().unwrap_or(ou(codigo, &numero_item)), 60);
    let desconto = desconto_item(i);
    let icms = match (emitente.crt == "3", cfop == "5405") {
        (true, true) => "60",
        (true, false) => "00",
        (false, true) => "500",
        (false, false) => "102",
    };
    FocusItem {
        numero_item,
        codigo_produto,
        codigo_barras: ean,
        descricao: take_chars(ou(&i.nome, "Produto").trim(), 120),
        cfop,
        codigo_ncm: ncm.clone(),
        ncm,
        unidade_comercial: unidade.clone(),
        quantidade_comercial: to_fixed(qty, 4),
        valor_unitario_comercial: to_fixed(i.preco, 2),
        valor_bruto: to_fixed(qty * i.preco, 2),
        unidade_tributavel: unidade,
        quantidade_tributavel: to_fixed(qty, 4),
        valor_unitario_tributavel: to_fixed(i.preco, 2),
        valor_desconto: (desconto > 0.0).then(|| to_fixed(desconto, 2)),
        inclui_no_total: "1".into(),
        icms_origem: "0".into(),
        icms_situacao_tributaria: icms.into(),
        pis_situacao_tributaria: "07".into(),
        cofins_situacao_tributaria: "07".into(),
        codigo_cest: sanitize_cest(&i.cest),
    }
}

fn montar_base(emitente: &Emitente, venda: &Venda, serie: &str, agora: DateTime<Utc>) -> NotaMontada {
    // vNF = soma dos vProd arredondados - vDesc
    let valor_total: f64 = venda.itens.iter().map(|i| bruto_item(i) - desconto_item(i)).sum();
    let doc = digits(&venda.destinatario.documento);
    let identificado = doc.len() == 11 || doc.len() == 14;
    let nome = venda.destinatario.nome.trim();
    let payload = FocusPayload {
        cnpj_emitente: digits(&emitente.cnpj),
        data_emissao: data_emissao(agora),
        natureza_operacao: "VENDA AO CONSUMIDOR".into(),
        tipo_documento: "1".into(),     // saída
        finalidade_emissao: "1".into(), // normal
        consumidor_final: "1".into(),
        presenca_comprador: "1".into(), // presencial
        modalidade_frete: "9".into(),   // sem transporte
        local_destino: "1".into(),      // operação interna
        serie: ou(serie, "1").to_string(),
        referencia: None,
        itens: venda.itens.iter().enumerate().map(|(n, i)| montar_item(emitente, i, n)).collect(),
        formas_pagamento: vec![FormaPagamento {
            forma_pagamento: codigo_forma_pagamento(&venda.forma_pagamento).into(),
            valor_pagamento: to_fixed(valor_total, 2),
        }],
        cpf_destinatario: (doc.len() == 11).then(|| doc.clone()),
        cnpj_destinatario: (doc.len() == 14).then(|| doc.clone()),
        // Nome só junto com CPF/CNPJ: destinatário sem documento é rejeitado pelo schema da SEFAZ
        nome_destinatario: (identificado && !nome.is_empty()).then(|| take_chars(nome, 60)),
        endereco: None,
    };
    NotaMontada { payload, valor_total }
}

/// Monta a NFC-e. Valida itens, NCM, documento do consumidor e o limite de R$ 10.000,00 sem identificação.
pub fn montar_nfce(emitente: &Emitente, venda: &Venda, agora: DateTime<Utc>) -> Result<NotaMontada, FiscalError> {
    validar_itens(venda)?;
    let nota = montar_base(emitente, venda, &emitente.serie_nfce, agora);
    if digits(&venda.destinatario.documento).is_empty() && nota.valor_total >= NFCE_LIMITE_SEM_IDENTIFICACAO {
        return Err(FiscalError::validacao(
            "Para NFC-e a partir de R$ 10.000,00 a SEFAZ exige o CPF ou CNPJ do consumidor.",
        ));
    }
    Ok(nota)
}

/// Monta a NF-e. O endereço do destinatário que faltar vem do emitente (a Focus devolve 422 sem ele).
pub fn montar_nfe(
    emitente: &Emitente,
    venda: &Venda,
    referencia: &str,
    agora: DateTime<Utc>,
) -> Result<NotaMontada, FiscalError> {
    validar_itens(venda)?;
    let mut nota = montar_base(emitente, venda, &emitente.serie_nfe, agora);
    nota.payload.referencia = Some(referencia.to_string());
    let d = &venda.destinatario;
    let campo = |dest: &str, emit: &str, padrao: &str| take_chars(ou(ou(dest.trim(), emit.trim()), padrao), 60);
    let cep = digits(&d.cep);
    let cep = if cep.is_empty() { digits(&emitente.cep) } else { cep };
    let complemento = d.complemento.trim();
    nota.payload.endereco = Some(EnderecoDestinatario {
        logradouro_destinatario: campo(&d.logradouro, &emitente.logradouro, "Rua Principal"),
        numero_destinatario: campo(&d.numero, &emitente.numero, "S/N"),
        complemento_destinatario: (!complemento.is_empty()).then(|| take_chars(complemento, 60)),
        bairro_destinatario: campo(&d.bairro, &emitente.bairro, "Centro"),
        municipio_destinatario: campo(&d.municipio, &emitente.municipio, "São Paulo"),
        uf_destinatario: take_chars(&ou(ou(d.uf.trim(), emitente.uf.trim()), "SP").to_uppercase(), 2),
        cep_destinatario: take_chars(ou(&cep, "01001000"), 8),
        indicador_inscricao_estadual_destinatario: "9".into(), // não contribuinte
    });
    Ok(nota)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn emitente() -> Emitente {
        Emitente {
            cnpj: "36.275.163/0001-24".into(),
            crt: "1".into(),
            serie_nfce: "1".into(),
            serie_nfe: "2".into(),
            cep: "66.000-000".into(),
            logradouro: "Av. Brasil".into(),
            numero: "100".into(),
            bairro: "Umarizal".into(),
            municipio: "Belém".into(),
            uf: "pa".into(),
        }
    }

    fn item(nome: &str, qtd: f64, preco: f64) -> ItemNota {
        ItemNota { nome: nome.into(), quantidade: qtd, preco, ncm: "2203.00.00".into(), ..Default::default() }
    }

    fn agora() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 18, 2, 5, 9).unwrap()
    }

    #[test]
    fn rateio_bate_com_o_desconto() {
        assert_eq!(ratear_desconto(&[10.0, 10.0, 10.0], 10.0), vec![3.33, 3.33, 3.34]);
        assert_eq!(ratear_desconto(&[5.0, 0.01], 5.01), vec![5.0, 0.01]);
        assert_eq!(ratear_desconto(&[5.0], 9.0), vec![5.0]); // nunca acima do total
        assert_eq!(ratear_desconto(&[5.0, 5.0], 0.0), vec![0.0, 0.0]);
        assert_eq!(ratear_desconto(&[], 3.0), Vec::<f64>::new());
        let partes = ratear_desconto(&[7.9, 12.5, 3.35, 0.99], 4.17);
        assert_eq!(round2(partes.iter().sum()), 4.17);
        assert!(partes.iter().zip([7.9, 12.5, 3.35, 0.99]).all(|(d, g)| *d <= g));
    }

    #[test]
    fn formas_de_pagamento() {
        assert_eq!(codigo_forma_pagamento("pix"), "17");
        assert_eq!(codigo_forma_pagamento("CREDIT_CARD"), "03");
        assert_eq!(codigo_forma_pagamento("cartão de crédito"), "03");
        assert_eq!(codigo_forma_pagamento("DEBIT_CARD"), "04");
        assert_eq!(codigo_forma_pagamento("CREDIT_TAB"), "05");
        assert_eq!(codigo_forma_pagamento("CASH"), "01");
        assert_eq!(codigo_forma_pagamento("qualquer"), "01");
    }

    #[test]
    fn referencias() {
        assert_eq!(referencia_nfce_com(Some("ckv9/x1!"), Some("bal7"), 1, 1234), "nfce_ckv9x1");
        assert_eq!(referencia_nfce_com(Some("!!"), Some("bal 7"), 1, 1234), "nfce_bal7_1234");
        assert_eq!(referencia_nfce_com(None, None, 1792337230588, 5555), "nfce_1792337230588_5555");
        assert_eq!(referencia_nfe_com(Some("pedido#42"), 1, 1000), "nfe_pedido42_1000");
        assert_eq!(referencia_nfe_com(None, 99, 1000), "nfe_99_1000");
        let r = referencia_nfce(None, Some("x"));
        let rnd: u32 = r.rsplit('_').next().unwrap().parse().unwrap();
        assert!((1000..10000).contains(&rnd));
    }

    #[test]
    fn data_em_brasilia() {
        assert_eq!(data_emissao(agora()), "2026-10-17T23:05:09-03:00");
    }

    #[test]
    fn nfce_completa() {
        let mut cerveja = item("  Cerveja 600ml ", 3.0, 12.5);
        cerveja.ean = "7891234567895".into();
        cerveja.desconto = 2.0;
        cerveja.cest = "302103".into();
        let mut porcao = item("Porção", 1.0, 0.125);
        porcao.cfop = "5.405".into();
        porcao.codigo = " P01 ".into();
        porcao.unidade = "porcao_grande".into();
        let venda = Venda {
            itens: vec![cerveja, porcao],
            destinatario: Destinatario { documento: "529.982.247-25".into(), nome: " Ana ".into(), ..Default::default() },
            forma_pagamento: "CREDIT_CARD".into(),
        };
        let nota = montar_nfce(&emitente(), &venda, agora()).unwrap();
        assert_eq!(nota.valor_total, 37.5 - 2.0 + 0.13);
        let v = serde_json::to_value(&nota.payload).unwrap();
        assert_eq!(
            v,
            json!({
                "cnpj_emitente": "36275163000124",
                "data_emissao": "2026-10-17T23:05:09-03:00",
                "natureza_operacao": "VENDA AO CONSUMIDOR",
                "tipo_documento": "1",
                "finalidade_emissao": "1",
                "consumidor_final": "1",
                "presenca_comprador": "1",
                "modalidade_frete": "9",
                "local_destino": "1",
                "serie": "1",
                "itens": [
                    {
                        "numero_item": "1",
                        "codigo_produto": "7891234567895",
                        "codigo_barras": "7891234567895",
                        "descricao": "Cerveja 600ml",
                        "cfop": "5102",
                        "codigo_ncm": "22030000",
                        "ncm": "22030000",
                        "unidade_comercial": "UN",
                        "quantidade_comercial": "3.0000",
                        "valor_unitario_comercial": "12.50",
                        "valor_bruto": "37.50",
                        "unidade_tributavel": "UN",
                        "quantidade_tributavel": "3.0000",
                        "valor_unitario_tributavel": "12.50",
                        "valor_desconto": "2.00",
                        "inclui_no_total": "1",
                        "icms_origem": "0",
                        "icms_situacao_tributaria": "102",
                        "pis_situacao_tributaria": "07",
                        "cofins_situacao_tributaria": "07",
                        "codigo_cest": "0302103"
                    },
                    {
                        "numero_item": "2",
                        "codigo_produto": "P01",
                        "descricao": "Porção",
                        "cfop": "5405",
                        "codigo_ncm": "22030000",
                        "ncm": "22030000",
                        "unidade_comercial": "PORCAO",
                        "quantidade_comercial": "1.0000",
                        "valor_unitario_comercial": "0.13",
                        "valor_bruto": "0.13",
                        "unidade_tributavel": "PORCAO",
                        "quantidade_tributavel": "1.0000",
                        "valor_unitario_tributavel": "0.13",
                        "inclui_no_total": "1",
                        "icms_origem": "0",
                        "icms_situacao_tributaria": "500",
                        "pis_situacao_tributaria": "07",
                        "cofins_situacao_tributaria": "07"
                    }
                ],
                "formas_pagamento": [{ "forma_pagamento": "03", "valor_pagamento": "35.63" }],
                "cpf_destinatario": "52998224725",
                "nome_destinatario": "Ana"
            })
        );
    }

    #[test]
    fn nfce_regime_normal_e_sem_documento() {
        let mut e = emitente();
        e.crt = "3".into();
        let mut st = item("Água", 1.0, 3.0);
        st.cfop = "5405".into();
        let venda = Venda {
            itens: vec![item("Suco", 0.0, 8.0), st],
            destinatario: Destinatario { nome: "Sem doc".into(), ..Default::default() },
            forma_pagamento: "PIX".into(),
        };
        let p = montar_nfce(&e, &venda, agora()).unwrap().payload;
        assert_eq!(p.itens[0].quantidade_comercial, "1.0000"); // quantidade 0 vira 1
        assert_eq!(p.itens[0].codigo_produto, "1"); // sem EAN nem código: número do item
        assert_eq!(p.itens[0].icms_situacao_tributaria, "00");
        assert_eq!(p.itens[1].icms_situacao_tributaria, "60");
        assert_eq!(p.nome_destinatario, None);
        assert_eq!(p.cpf_destinatario, None);
        assert_eq!(p.formas_pagamento[0].forma_pagamento, "17");
    }

    #[test]
    fn validacoes() {
        let erro = |venda: &Venda| montar_nfce(&emitente(), venda, agora()).unwrap_err().to_string();
        assert_eq!(erro(&Venda::default()), "Nenhum item adicionado para a nota.");

        let mut sem_ncm = item("Batata", 1.0, 10.0);
        sem_ncm.ncm = "2004100".into();
        let venda = Venda { itens: vec![item("Ok", 1.0, 1.0), sem_ncm], ..Default::default() };
        assert_eq!(erro(&venda), "O produto \"Batata\" está sem NCM válido (8 dígitos). Cadastre o NCM antes de emitir.");

        let venda = Venda {
            itens: vec![item("Ok", 1.0, 1.0)],
            destinatario: Destinatario { documento: "529.982.247-24".into(), ..Default::default() },
            ..Default::default()
        };
        assert_eq!(erro(&venda), "CPF com dígitos verificadores inválidos.");

        let venda = Venda { itens: vec![item("Whisky", 1.0, 10_000.0)], ..Default::default() };
        assert!(erro(&venda).contains("a SEFAZ exige o CPF ou CNPJ"));
        let mut venda = venda;
        venda.destinatario.documento = "36275163000124".into();
        let p = montar_nfce(&emitente(), &venda, agora()).unwrap().payload;
        assert_eq!(p.cnpj_destinatario.as_deref(), Some("36275163000124"));
    }

    #[test]
    fn nfe_com_endereco_do_emitente() {
        let venda = Venda {
            itens: vec![item("Barril", 2.0, 350.0)],
            destinatario: Destinatario {
                documento: "36275163000124".into(),
                nome: "Bar do Zé".into(),
                logradouro: "  ".into(),
                numero: "12".into(),
                complemento: "Sala 2".into(),
                ..Default::default()
            },
            forma_pagamento: "DEBIT_CARD".into(),
        };
        let nota = montar_nfe(&emitente(), &venda, "nfe_7_1234", agora()).unwrap();
        let v = serde_json::to_value(&nota.payload).unwrap();
        assert_eq!(v["referencia"], "nfe_7_1234");
        assert_eq!(v["serie"], "2");
        assert_eq!(v["nome_destinatario"], "Bar do Zé");
        assert_eq!(v["logradouro_destinatario"], "Av. Brasil");
        assert_eq!(v["numero_destinatario"], "12");
        assert_eq!(v["complemento_destinatario"], "Sala 2");
        assert_eq!(v["bairro_destinatario"], "Umarizal");
        assert_eq!(v["municipio_destinatario"], "Belém");
        assert_eq!(v["uf_destinatario"], "PA");
        assert_eq!(v["cep_destinatario"], "66000000");
        assert_eq!(v["indicador_inscricao_estadual_destinatario"], "9");
        assert_eq!(v["formas_pagamento"][0], json!({ "forma_pagamento": "04", "valor_pagamento": "700.00" }));
        assert_eq!(nota.valor_total, 700.0);
    }

    #[test]
    fn nfe_sem_dados_usa_padroes() {
        let venda = Venda { itens: vec![item("X", 1.0, 1.0)], ..Default::default() };
        let v = serde_json::to_value(montar_nfe(&Emitente::default(), &venda, "r", agora()).unwrap().payload).unwrap();
        assert_eq!(v["logradouro_destinatario"], "Rua Principal");
        assert_eq!(v["numero_destinatario"], "S/N");
        assert_eq!(v["municipio_destinatario"], "São Paulo");
        assert_eq!(v["uf_destinatario"], "SP");
        assert_eq!(v["cep_destinatario"], "01001000");
        assert_eq!(v["serie"], "1");
        assert!(v.get("complemento_destinatario").is_none());
    }
}
