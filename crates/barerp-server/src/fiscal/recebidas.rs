//! Notas de compra: importação do XML, entrada no estoque, bip da chave de acesso, lista de pendentes e
//! sincronização com as notas que a SEFAZ registrou contra o CNPJ.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use barerp_db::importacao::{self, FornecedorNfe, ImportacaoXml};
use barerp_db::notas::{self, FiltroRecebidas, LadoCofre, RegistroRecebida};
use barerp_fiscal::{ler_nfe_xml, parece_xml_nfe, parse_chave_acesso, NfeXml, XmlRecebida};
use serde_json::{json, Value};

use super::{agendador, agora_iso, opt, so_digitos, txt, FiscalState};
use crate::error::{ApiError, ApiResult};

fn fornecedor_do_xml(n: &NfeXml) -> FornecedorNfe {
    let e = &n.emitente;
    FornecedorNfe {
        documento: e.documento.clone(),
        nome: e.nome.clone(),
        fantasia: e.fantasia.clone(),
        ie: e.ie.clone(),
        telefone: e.fone.clone(),
        endereco: e.endereco.clone(),
        cidade: e.municipio.clone(),
        uf: e.uf.clone(),
    }
}

fn arquivar_entrada(st: &FiscalState, chave: &str, xml: &str) {
    if let Err(e) = notas::arquivar_xml(&st.cofre(), LadoCofre::Entrada, chave, xml) {
        tracing::warn!("cofre de XML: não gravou a nota de entrada {chave}: {e}");
    }
}

/// XML guardado na nota (aceita o formato antigo `{"raw": ...}`).
fn xml_salvo(conteudo: Option<&str>) -> Option<String> {
    let c = conteudo?;
    if c.trim_start().starts_with('{') {
        return serde_json::from_str::<Value>(c).ok()?.get("raw")?.as_str().map(str::to_string);
    }
    parece_xml_nfe(c).then(|| c.to_string())
}

/// Itens do XML no formato da tela de conferência.
fn itens_json(n: &NfeXml, padrao_nome: Option<&str>, padrao_unidade: &str) -> Vec<Value> {
    n.itens
        .iter()
        .enumerate()
        .map(|(i, it)| {
            json!({
                "id": format!("xml_item_{i}"),
                "code": it.codigo.clone().unwrap_or_default(),
                "ean": it.ean,
                "name": if it.nome.is_empty() { padrao_nome.map(str::to_string) } else { Some(it.nome.clone()) },
                "quantity": it.quantidade,
                "unitCost": it.custo_unitario,
                "ncm": it.ncm.clone().unwrap_or_default(),
                "cfop": it.cfop.clone().unwrap_or_default(),
                "unit": if it.unidade.is_empty() { padrao_unidade.to_string() } else { it.unidade.clone() },
            })
        })
        .collect()
}

fn registro_do_xml(chave: &str, n: &NfeXml, xml: &str) -> RegistroRecebida {
    RegistroRecebida {
        chave: chave.to_string(),
        emitente: Some(n.emitente.nome.clone()),
        cnpj_emitente: Some(n.emitente.documento.clone()),
        numero: Some(n.numero.clone()),
        serie: Some(n.serie.clone()),
        data_emissao: n.data_emissao.clone(),
        valor_total: Some(n.valor_total),
        xml_content: Some(xml.to_string()),
    }
}

// ============================================================
// Importação do arquivo XML e entrada no estoque
// ============================================================

pub(super) async fn import_xml(State(st): State<FiscalState>, mut m: Multipart) -> ApiResult<Json<Value>> {
    let mut arquivo: Option<Vec<u8>> = None;
    while let Some(campo) = m.next_field().await.map_err(|e| ApiError::bad_request(format!("Envio inválido: {e}")))? {
        if campo.name() == Some("xml") {
            arquivo = Some(campo.bytes().await.map_err(|e| ApiError::bad_request(format!("Envio inválido: {e}")))?.to_vec());
        }
    }
    let Some(bytes) = arquivo else {
        return Err(ApiError::bad_request("Nenhum arquivo enviado."));
    };
    let xml = String::from_utf8_lossy(&bytes).into_owned();
    let n = ler_nfe_xml(&xml).map_err(|_| ApiError::bad_request("O arquivo não parece ser um XML válido de NF-e."))?;

    let fornecedor_id = importacao::upsert_fornecedor_nfe(&st.db, &fornecedor_do_xml(&n)).await?;
    let data_emissao = n.data_emissao.clone().unwrap_or_else(agora_iso);
    let chave = so_digitos(&n.chave_acesso);
    if !chave.is_empty() {
        // para o fechamento fiscal do mês e o SPED; "finalizada" não volta atrás
        let reg = RegistroRecebida { data_emissao: Some(data_emissao.clone()), ..registro_do_xml(&chave, &n, &xml) };
        notas::gravar_recebida(&st.db, &reg, Some("recebida")).await?;
        arquivar_entrada(&st, &chave, &xml);
    }
    Ok(Json(json!({
        "vendor": {
            "id": fornecedor_id,
            "name": n.emitente.nome,
            "tradeName": n.emitente.fantasia,
            "cnpj": n.emitente.documento,
        },
        "items": itens_json(&n, None, "un"),
        "accessKey": n.chave_acesso,
        "numero": n.numero,
        "serie": n.serie,
        "dataEmissao": data_emissao,
        "valorTotal": n.valor_total,
    })))
}

pub(super) async fn apply_import(State(st): State<FiscalState>, headers: HeaderMap, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    // a forma antiga mandava só o array de itens
    let corpo = if body.is_array() { json!({ "items": body }) } else { body };
    if !corpo.get("items").is_some_and(Value::is_array) {
        return Err(ApiError::bad_request("Nenhum item para importar."));
    }
    let imp: ImportacaoXml = serde_json::from_value(corpo).map_err(|e| ApiError::bad_request(format!("Itens inválidos: {e}")))?;
    let usuario = headers.get("x-user-name").and_then(|v| v.to_str().ok());
    let results = importacao::aplicar_importacao_xml(&st.db, &imp, usuario).await?;
    Ok(Json(json!({ "success": true, "results": results })))
}

// ============================================================
// Bip da chave de acesso
// ============================================================

/// Dados da nota pelo XML completo, ou pelo JSON da Focus e pela chave quando o XML ainda não saiu.
struct DadosNota {
    emitente: String,
    cnpj: String,
    numero: String,
    serie: String,
    data_emissao: String,
    valor: f64,
}

fn dados_da_nota(chave: &str, r: Option<&XmlRecebida>) -> (DadosNota, Option<NfeXml>) {
    let k = parse_chave_acesso(chave);
    let j = r.and_then(|r| r.dados.clone()).unwrap_or(Value::Null);
    let mut d = DadosNota {
        emitente: txt(&j, "nome_emitente"),
        cnpj: so_digitos(&opt(&j, "documento_emitente").or_else(|| opt(&j, "cnpj_emitente")).unwrap_or_else(|| {
            k.as_ref().map(|k| k.cnpj_emitente.clone()).unwrap_or_default()
        })),
        numero: opt(&j, "numero").or_else(|| k.as_ref().map(|k| k.numero.clone())).unwrap_or_default(),
        serie: opt(&j, "serie").or_else(|| k.as_ref().map(|k| k.serie.clone())).unwrap_or_default(),
        data_emissao: opt(&j, "data_emissao").unwrap_or_else(agora_iso),
        valor: txt(&j, "valor_total").parse().unwrap_or(0.0),
    };
    let xml = r.and_then(|r| r.xml.as_deref()).and_then(|x| ler_nfe_xml(x).ok());
    if let Some(n) = &xml {
        if !n.emitente.nome.is_empty() && n.emitente.nome != "Fornecedor sem nome" {
            d.emitente = n.emitente.nome.clone();
        }
        if !n.emitente.documento.is_empty() {
            d.cnpj = n.emitente.documento.clone();
        }
        if !n.numero.is_empty() {
            d.numero = n.numero.clone();
        }
        if !n.serie.is_empty() {
            d.serie = n.serie.clone();
        }
        if let Some(dh) = &n.data_emissao {
            d.data_emissao = dh.clone();
        }
        if n.valor_total > 0.0 {
            d.valor = n.valor_total;
        }
    }
    if d.emitente.is_empty() {
        d.emitente = format!("Fornecedor CNPJ {}", k.map(|k| k.cnpj_formatado).unwrap_or_default());
    }
    (d, xml)
}

pub(super) async fn bip_chave(State(st): State<FiscalState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let chave = so_digitos(&txt(&body, "chave"));
    if chave.len() != 44 {
        return Err(ApiError::bad_request("Chave de acesso inválida. Deve ter exatamente 44 dígitos numéricos."));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Configure o Token da API Fiscal nas Configurações Fiscais antes de usar o bip.")?;

    // 1. Ciência da operação na SEFAZ
    let manifesto = focus.manifestar_ciencia(&chave).await;
    let manifesto_ok = match &manifesto {
        Ok(_) => true,
        Err(barerp_fiscal::FiscalError::Focus { detalhes, .. }) => detalhes.to_string().contains("ciencia"),
        Err(_) => false,
    };
    // a Focus e a SEFAZ levam um instante para registrar o evento
    tokio::time::sleep(st.esperas.bip).await;

    // 2. XML e dados da nota
    let resultado = focus.baixar_xml_recebida(&chave).await;
    let xml = resultado.as_ref().and_then(|r| r.xml.clone());
    let (d, lido) = dados_da_nota(&chave, resultado.as_ref());
    let status = if xml.is_some() { "recebida" } else { "ciencia_registrada" };

    // 3. Registro local. Sem o XML guarda o JSON da Focus, mas nunca por cima de um XML completo já salvo.
    let ja_tem_xml = notas::buscar_recebida(&st.db, &chave).await?.is_some_and(|n| n.tem_xml());
    let conteudo = xml.clone().or_else(|| {
        (!ja_tem_xml).then(|| resultado.as_ref().and_then(|r| r.dados.as_ref()).map(Value::to_string)).flatten()
    });
    let reg = RegistroRecebida {
        chave: chave.clone(),
        emitente: Some(d.emitente.clone()),
        cnpj_emitente: Some(d.cnpj.clone()),
        numero: Some(d.numero.clone()),
        serie: Some(d.serie.clone()),
        data_emissao: Some(d.data_emissao.clone()),
        valor_total: Some(d.valor),
        xml_content: conteudo,
    };
    notas::gravar_recebida(&st.db, &reg, Some(status)).await?;
    let fornecedor = match &lido {
        Some(n) => fornecedor_do_xml(n),
        None => FornecedorNfe { documento: d.cnpj.clone(), nome: d.emitente.clone(), ..Default::default() },
    };
    importacao::upsert_fornecedor_nfe(&st.db, &fornecedor).await?;
    if let Some(x) = &xml {
        arquivar_entrada(&st, &chave, x);
    }
    let status_final = notas::buscar_recebida(&st.db, &chave).await?.map_or(status.to_string(), |n| n.status);
    Ok(Json(json!({
        "success": true,
        "chave": chave,
        "emitente": d.emitente,
        "cnpjEmitente": d.cnpj,
        "numero": d.numero,
        "serie": d.serie,
        "dataEmissao": d.data_emissao,
        "valorTotal": d.valor,
        "xmlDisponivel": xml.is_some(),
        "status": status_final,
        "manifestoRegistrado": manifesto_ok,
        "mensagem": if xml.is_some() {
            "✓ 1º Bip Concluído! Ciência registrada na SEFAZ e XML baixado com sucesso. A nota está pronta para o 2º Bip."
        } else {
            "✓ 1º Bip Concluído! Ciência da Operação registrada na SEFAZ. O download do XML completo está sendo processado pelos servidores da SEFAZ (aguarde ~30s ou faça upload do XML direto)."
        },
    })))
}

// ============================================================
// Lista e ações rápidas
// ============================================================

pub(super) async fn notas_pendentes(State(st): State<FiscalState>, Query(f): Query<FiltroRecebidas>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(notas::listar_recebidas(&st.db, &f).await?)))
}

pub(super) async fn notas_recebidas(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(notas::recebidas_recentes(&st.db).await?)))
}

pub(super) async fn receber(State(st): State<FiscalState>, Path(chave): Path<String>) -> ApiResult<Json<Value>> {
    let chave = so_digitos(&chave);
    if chave.len() != 44 {
        return Err(ApiError::bad_request("Chave de acesso deve conter 44 dígitos."));
    }
    let mut xml = None;
    if let Some(focus) = st.focus(&st.config().await?) {
        if let Err(e) = focus.manifestar_ciencia(&chave).await {
            tracing::warn!("ciência da nota {chave}: {e}");
        }
        xml = focus.baixar_xml_recebida(&chave).await.and_then(|r| r.xml);
        if let Some(x) = &xml {
            arquivar_entrada(&st, &chave, x);
        }
    }
    notas::receber_recebida(&st.db, &chave, xml.as_deref()).await?;
    Ok(Json(json!({
        "success": true,
        "message": if xml.is_some() {
            "✓ Nota recebida com sucesso (1º Bip)! XML baixado e arquivado."
        } else {
            "✓ Nota recebida com sucesso (1º Bip)! Ciência registrada na SEFAZ."
        },
        "hasXml": xml.is_some(),
    })))
}

pub(super) async fn finalizar(State(st): State<FiscalState>, Path(chave): Path<String>) -> ApiResult<Json<Value>> {
    if so_digitos(&chave).is_empty() {
        return Err(ApiError::bad_request("Chave de acesso inválida."));
    }
    notas::finalizar_recebida(&st.db, &chave).await?;
    Ok(Json(json!({ "success": true, "message": "✓ Nota marcada como finalizada (Entrada no estoque confirmada)." })))
}

pub(super) async fn reabrir(State(st): State<FiscalState>, Path(chave): Path<String>) -> ApiResult<Json<Value>> {
    if so_digitos(&chave).is_empty() {
        return Err(ApiError::bad_request("Chave de acesso inválida."));
    }
    notas::reabrir_recebida(&st.db, &chave).await?;
    Ok(Json(json!({ "success": true, "message": "✓ Nota reaberta para conferência (2º Bip)." })))
}

// ============================================================
// XML de uma nota recebida
// ============================================================

pub(super) async fn sync_nota(State(st): State<FiscalState>, Path(chave): Path<String>) -> ApiResult<Json<Value>> {
    let chave = so_digitos(&chave);
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da Focus NFe não configurado.")?;
    if let Err(e) = focus.manifestar_ciencia(&chave).await {
        tracing::warn!("ciência da nota {chave}: {e}");
    }
    let resultado = focus.baixar_xml_recebida(&chave).await;
    if let Some(xml) = resultado.as_ref().and_then(|r| r.xml.clone()) {
        let (d, _) = dados_da_nota(&chave, resultado.as_ref());
        let reg = RegistroRecebida {
            chave: chave.clone(),
            emitente: Some(d.emitente),
            cnpj_emitente: Some(d.cnpj),
            numero: Some(d.numero),
            serie: Some(d.serie),
            data_emissao: None,
            valor_total: Some(d.valor),
            xml_content: Some(xml.clone()),
        };
        // "finalizada" não regride (o Node gravava "recebida" por cima)
        notas::gravar_recebida(&st.db, &reg, Some("recebida")).await?;
        arquivar_entrada(&st, &chave, &xml);
        return Ok(Json(json!({
            "success": true,
            "xmlDisponivel": true,
            "message": "✓ XML sincronizado com sucesso da SEFAZ! A nota está pronta para o 2º Bip / Conferência.",
        })));
    }
    let dados = resultado.and_then(|r| r.dados).unwrap_or(Value::Null);
    Ok(Json(json!({
        "success": false,
        "xmlDisponivel": false,
        "statusSefaz": opt(&dados, "status").or_else(|| opt(&dados, "manifesto")).unwrap_or_else(|| "pendente".into()),
        "message": "A SEFAZ ainda não liberou o download do XML para esta nota. Você pode aguardar mais um momento ou enviar o arquivo .xml manualmente.",
    })))
}

fn resposta_xml(xml: String) -> Response {
    ([(header::CONTENT_TYPE, "application/xml")], xml).into_response()
}

pub(super) async fn xml_nota(State(st): State<FiscalState>, Path(chave): Path<String>) -> ApiResult<Response> {
    let chave = so_digitos(&chave);
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API não configurado.")?;
    if let Some(xml) = notas::buscar_recebida(&st.db, &chave).await?.and_then(|n| xml_salvo(n.xml_content.as_deref())) {
        return Ok(resposta_xml(xml));
    }
    match focus.baixar_xml_recebida(&chave).await.and_then(|r| r.xml) {
        Some(xml) => Ok(resposta_xml(xml)),
        None => Err(ApiError::not_found(
            "XML não disponível na SEFAZ ainda. Aguarde alguns instantes ou faça upload do arquivo .xml recebido do fornecedor.",
        )),
    }
}

pub(super) async fn parsed_by_chave(State(st): State<FiscalState>, Path(chave): Path<String>) -> ApiResult<Json<Value>> {
    let chave = so_digitos(&chave);
    if chave.len() != 44 {
        return Err(ApiError::bad_request("Chave de acesso deve conter exatamente 44 dígitos."));
    }
    let local = notas::buscar_recebida(&st.db, &chave).await?;
    let mut xml = local.as_ref().and_then(|n| xml_salvo(n.xml_content.as_deref()));
    let mut baixado_agora = false;
    if xml.is_none() {
        let cfg = st.config().await?;
        let focus =
            st.focus_ou(&cfg, "Token da API Fiscal não configurado. Verifique as configurações fiscais.")?;
        if let Err(e) = focus.manifestar_ciencia(&chave).await {
            tracing::warn!("ciência da nota {chave}: {e}");
        }
        xml = focus.baixar_xml_recebida(&chave).await.and_then(|r| r.xml);
        baixado_agora = xml.is_some();
    }
    let Some(xml) = xml else {
        return Err(ApiError::with_body(
            axum::http::StatusCode::NOT_FOUND,
            json!({
                "error": "XML da nota ainda em processamento na SEFAZ. Como a Ciência da Operação foi registrada agora, a SEFAZ leva cerca de 30 a 60 segundos para liberar o arquivo completo. Você também pode importar o arquivo .xml diretamente.",
                "xmlPending": true,
            }),
        ));
    };
    let n = ler_nfe_xml(&xml).map_err(|_| ApiError::bad_request("Não foi possível extrair dados válidos de NF-e do XML."))?;

    // a nota fica registrada com o valor e o XML corretos
    let status = (baixado_agora || local.is_none()).then_some("recebida");
    notas::gravar_recebida(&st.db, &registro_do_xml(&chave, &n, &xml), status).await?;
    let fornecedor_id = importacao::upsert_fornecedor_nfe(
        &st.db,
        &FornecedorNfe { documento: n.emitente.documento.clone(), nome: n.emitente.nome.clone(), ..Default::default() },
    )
    .await?;
    Ok(Json(json!({
        "vendor": {
            "id": fornecedor_id,
            "name": n.emitente.nome,
            "tradeName": n.emitente.fantasia,
            "cnpj": n.emitente.documento,
        },
        "items": itens_json(&n, Some("Produto sem descrição"), "UN"),
        "accessKey": chave,
        "numero": n.numero,
        "serie": n.serie,
        "dataEmissao": n.data_emissao,
        "valorTotal": n.valor_total,
    })))
}

pub(super) async fn sync_todas(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    let r = agendador::sincronizar_recebidas(&st).await?;
    let message = match (r.adicionadas, r.com_xml) {
        (0, 0) => "Sincronização concluída. Nenhuma nova nota ou XML pendente na SEFAZ.".to_string(),
        (a, 0) => format!("✓ {a} nova(s) nota(s) detectada(s) contra o seu CNPJ."),
        (0, x) => format!("✓ {x} arquivo(s) XML baixado(s) da SEFAZ com sucesso."),
        (a, x) => format!("✓ {a} nova(s) nota(s) detectada(s) e {x} arquivo(s) XML baixado(s) com sucesso."),
    };
    Ok(Json(json!({ "success": true, "count": r.adicionadas, "updatedWithXml": r.com_xml, "message": message })))
}
