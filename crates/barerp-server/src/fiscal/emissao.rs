//! Notas emitidas pelo bar: emissão de NFC-e e NF-e, cancelamento, carta de correção, inutilização,
//! e-mail, reimpressão, listas e o proxy do DANFE.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use barerp_db::importacao;
use barerp_db::notas::{self, NotaEmitida, RegistroEmitida, TipoNota};
use barerp_fiscal::{
    ajustar_danfe_nfce_html, montar_nfce, montar_nfe, parse_chave_acesso, referencia_nfce, referencia_nfe, AjustesDanfe,
    Destinatario, Emissao, FiscalError, FocusClient, ItemNota, Method, Modelo, Venda,
};
use chrono::{Datelike, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    agora_iso, arquivar_saida_em_segundo_plano, danfe_url, emitente, erro_fiscal, erro_repassado, num, opt, so_digitos,
    txt, uri_seg, FiscalState,
};
use crate::error::{ApiError, ApiResult};

fn modelo(tipo: TipoNota) -> Modelo {
    match tipo {
        TipoNota::Nfce => Modelo::Nfce,
        TipoNota::Nfe => Modelo::Nfe,
    }
}

fn venda_do_corpo(body: &Value) -> Venda {
    let itens = body
        .get("items")
        .and_then(Value::as_array)
        .map(|itens| {
            itens
                .iter()
                .map(|i| ItemNota {
                    nome: txt(i, "name"),
                    quantidade: num(i, "quantity"),
                    preco: num(i, "price"),
                    desconto: num(i, "discount"),
                    ncm: txt(i, "ncm"),
                    cfop: txt(i, "cfop"),
                    unidade: txt(i, "unit"),
                    ean: txt(i, "ean"),
                    codigo: txt(i, "code"),
                    cest: txt(i, "cest"),
                })
                .collect()
        })
        .unwrap_or_default();
    Venda {
        itens,
        destinatario: Destinatario {
            documento: txt(body, "customerCpf"),
            nome: txt(body, "customerName"),
            cep: txt(body, "customerCep"),
            logradouro: txt(body, "customerLogradouro"),
            numero: txt(body, "customerNumero"),
            complemento: txt(body, "customerComplemento"),
            bairro: txt(body, "customerBairro"),
            municipio: txt(body, "customerMunicipio"),
            uf: txt(body, "customerUf"),
        },
        forma_pagamento: txt(body, "paymentMethod"),
    }
}

fn sem_itens(body: &Value) -> bool {
    body.get("items").and_then(Value::as_array).is_none_or(|i| i.is_empty())
}

/// Consulta a Focus e grava o que voltou autorizado (notas que ficaram "processando" ou sem número).
async fn sincronizar(st: &FiscalState, focus: &FocusClient, nota: &mut NotaEmitida) {
    let Ok(r) = focus.consultar(modelo(nota.tipo()), &nota.referencia, true).await else {
        return;
    };
    if r.status() != "autorizado" {
        return;
    }
    let d = &r.corpo;
    let campo = |k: &str| match d.get(k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    nota.status = "autorizado".into();
    nota.chave = campo("chave_nfe").or_else(|| campo("chave")).or(nota.chave.take());
    nota.numero = campo("numero").or(nota.numero.take());
    nota.serie = campo("serie").or(nota.serie.take());
    let reg = RegistroEmitida {
        referencia: nota.referencia.clone(),
        chave: nota.chave.clone(),
        numero: nota.numero.clone(),
        serie: nota.serie.clone(),
        status: Some("autorizado".into()),
        ..Default::default()
    };
    if let Err(e) = notas::atualizar_emitida(&st.db, &reg).await {
        tracing::warn!("não gravou a sincronização da nota {}: {e}", nota.referencia);
    }
}

// ============================================================
// Emissão
// ============================================================

pub(super) async fn emit_nfce(
    State(st): State<FiscalState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    if sem_itens(&body) {
        return Err(ApiError::bad_request("Nenhum item adicionado para a nota."));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API Fiscal não configurado. Vá nas Configurações Fiscais.")?;
    let tem = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
    if !tem(&cfg.csc_secret) || !tem(&cfg.csc_id) {
        return Err(ApiError::bad_request(
            "Para emitir NFC-e, preencha o Código CSC e o ID do Token nas Configurações Fiscais.",
        ));
    }
    let sale_key = opt(&body, "saleKey");
    let referencia = referencia_nfce(sale_key.as_deref(), opt(&body, "orderId").as_deref());
    // saleKey = id da venda: referência fixa, então "tentar de novo" consulta antes e nunca duplica a nota
    let referencia_fixa =
        sale_key.is_some_and(|k| k.chars().any(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    let nota = montar_nfce(&emitente(&cfg), &venda_do_corpo(&body), Utc::now()).map_err(erro_fiscal)?;

    match focus.emitir_nfce(&referencia, &nota.payload, referencia_fixa).await.map_err(erro_fiscal)? {
        Emissao::Autorizada(n) => {
            let danfe = danfe_url(&headers, &n.referencia);
            let xml_url =
                n.caminho_xml.clone().unwrap_or_else(|| format!("{}/v2/nfce/{}.xml", focus.base_url(), n.referencia));
            notas::gravar_emitida(
                &st.db,
                &RegistroEmitida {
                    referencia: n.referencia.clone(),
                    chave: n.chave.clone(),
                    numero: n.numero.clone(),
                    serie: n.serie.clone(),
                    data_emissao: Some(agora_iso()),
                    valor_total: Some(nota.valor_total),
                    status: Some("autorizado".into()),
                    xml_url: Some(xml_url),
                    pdf_url: Some(danfe.clone()),
                },
            )
            .await?;
            let chave = n.chave.clone().unwrap_or_else(|| n.referencia.clone());
            arquivar_saida_em_segundo_plano(&st, &focus, n.caminho_xml.clone(), chave);
            Ok(Json(json!({
                "success": true,
                "status": "autorizado",
                "referencia": n.referencia,
                "chaveAcesso": n.chave,
                "caminhoDanfe": danfe,
            })))
        }
        Emissao::Processando { referencia, .. } => {
            notas::gravar_emitida(
                &st.db,
                &RegistroEmitida {
                    referencia: referencia.clone(),
                    valor_total: Some(nota.valor_total),
                    status: Some("processando".into()),
                    data_emissao: Some(agora_iso()),
                    ..Default::default()
                },
            )
            .await?;
            Ok(Json(json!({
                "success": true,
                "status": "processando",
                "mensagem": "A nota está na fila da SEFAZ. Você poderá consultar depois.",
                "referencia": referencia,
                "ref": referencia,
            })))
        }
    }
}

pub(super) async fn emit_nfe(
    State(st): State<FiscalState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    if sem_itens(&body) {
        return Err(ApiError::bad_request("Nenhum item adicionado para a nota."));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API Fiscal não configurado. Vá nas Configurações Fiscais.")?;
    let referencia = referencia_nfe(opt(&body, "orderId").as_deref());
    let nota = montar_nfe(&emitente(&cfg), &venda_do_corpo(&body), &referencia, Utc::now()).map_err(erro_fiscal)?;
    let danfe = danfe_url(&headers, &referencia);
    let serie_padrao = cfg.serie_nfe.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "1".into());
    let xml_padrao = format!("{}/v2/nfe/{referencia}.xml", focus.base_url());

    match focus.emitir_nfe(&referencia, &nota.payload).await {
        Ok(Emissao::Autorizada(n)) => {
            let serie = n.serie.clone().unwrap_or(serie_padrao);
            notas::gravar_emitida(
                &st.db,
                &RegistroEmitida {
                    referencia: referencia.clone(),
                    chave: n.chave.clone(),
                    numero: n.numero.clone(),
                    serie: Some(serie.clone()),
                    valor_total: Some(nota.valor_total),
                    status: Some("autorizado".into()),
                    xml_url: Some(n.caminho_xml.clone().unwrap_or(xml_padrao)),
                    pdf_url: Some(danfe.clone()),
                    ..Default::default()
                },
            )
            .await?;
            arquivar_saida_em_segundo_plano(
                &st,
                &focus,
                n.caminho_xml.clone(),
                n.chave.clone().unwrap_or_else(|| referencia.clone()),
            );
            Ok(Json(json!({
                "success": true,
                "status": "autorizado",
                "referencia": referencia,
                "chaveAcesso": n.chave,
                "caminhoDanfe": danfe,
                "numero": n.numero,
                "serie": serie,
            })))
        }
        Ok(Emissao::Processando { dados, .. }) => {
            let campo = |k: &str| dados.get(k).filter(|v| !v.is_null()).map(|v| v.as_str().map_or(v.to_string(), str::to_string));
            let serie = campo("serie").unwrap_or(serie_padrao);
            let chave = campo("chave_nfe").or_else(|| campo("chave"));
            // fica no histórico e nas telas de consulta e cancelamento
            notas::gravar_emitida(
                &st.db,
                &RegistroEmitida {
                    referencia: referencia.clone(),
                    chave: chave.clone(),
                    numero: campo("numero"),
                    serie: Some(serie.clone()),
                    valor_total: Some(nota.valor_total),
                    status: Some("processando".into()),
                    xml_url: Some(campo("caminho_xml_nota_fiscal").unwrap_or(xml_padrao)),
                    pdf_url: Some(danfe.clone()),
                    ..Default::default()
                },
            )
            .await?;
            Ok(Json(json!({
                "success": true,
                "status": "processando",
                "mensagem": "A NF-e está na fila de processamento da SEFAZ. O status será atualizado automaticamente.",
                "referencia": referencia,
                "caminhoDanfe": danfe,
                "chaveAcesso": chave,
                "numero": campo("numero"),
                "serie": serie,
            })))
        }
        Err(FiscalError::Sefaz { status, mensagem, .. }) => {
            // recusada pela SEFAZ: grava com o status para aparecer no histórico
            notas::gravar_emitida(
                &st.db,
                &RegistroEmitida {
                    referencia: referencia.clone(),
                    valor_total: Some(nota.valor_total),
                    status: Some(status),
                    ..Default::default()
                },
            )
            .await?;
            Err(ApiError::bad_request(mensagem))
        }
        Err(e) => Err(erro_fiscal(e)),
    }
}

// ============================================================
// Cancelamento, carta de correção, inutilização, e-mail
// ============================================================

pub(super) async fn cancel_nfce(State(st): State<FiscalState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let referencia = txt(&body, "referencia");
    if referencia.is_empty() {
        return Err(ApiError::bad_request("Referência (ID do Pedido) não informada."));
    }
    if txt(&body, "justificativa").trim().chars().count() < 15 {
        return Err(ApiError::bad_request("A justificativa deve ter no mínimo 15 caracteres (Regra da SEFAZ)."));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API não configurado.")?;
    let data = focus
        .cancelar(Modelo::Nfce, &referencia, &txt(&body, "justificativa"))
        .await
        .map_err(|e| erro_repassado(e, ""))?;
    notas::marcar_emitida_cancelada(&st.db, &referencia, None).await?;
    Ok(Json(json!({ "ok": true, "mensagem": "Nota fiscal cancelada com sucesso!", "data": data })))
}

pub(super) async fn cancel_nfe(State(st): State<FiscalState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let alvo = [txt(&body, "referencia"), txt(&body, "chave"), txt(&body, "numero")]
        .into_iter()
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty());
    let Some(alvo) = alvo else {
        return Err(ApiError::bad_request("Referência, Chave ou Número da NF-e não informado."));
    };
    let justificativa = txt(&body, "justificativa");
    if justificativa.trim().chars().count() < 15 {
        return Err(ApiError::bad_request("A justificativa deve ter no mínimo 15 caracteres (Regra da SEFAZ)."));
    }
    // a tela pode mandar o número ou a chave: acha a referência real no banco
    let local = match notas::buscar_emitida(&st.db, &alvo).await? {
        Some(n) => Some(n),
        None => notas::buscar_emitidas_por_numero(&st.db, &alvo, None)
            .await?
            .into_iter()
            .find(|n| n.tipo() == TipoNota::Nfe),
    };
    let referencia = local.as_ref().map_or(alvo.clone(), |n| n.referencia.clone());
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API não configurado.")?;
    let mut res = focus.cancelar(Modelo::Nfe, &referencia, &justificativa).await;
    // falhou pela referência: tenta pela chave
    if let (Err(FiscalError::Focus { .. }), Some(chave)) = (&res, local.as_ref().and_then(|n| n.chave.clone())) {
        if chave != referencia {
            if let Ok(d) = focus.cancelar(Modelo::Nfe, &chave, &justificativa).await {
                res = Ok(d);
            }
        }
    }
    let data = res.map_err(|e| erro_repassado(e, ""))?;
    let chave = local.as_ref().and_then(|n| n.chave.as_deref());
    notas::marcar_emitida_cancelada(&st.db, &referencia, chave).await?;
    if alvo != referencia {
        notas::marcar_emitida_cancelada(&st.db, &alvo, None).await?;
    }
    Ok(Json(json!({ "ok": true, "mensagem": "NF-e cancelada com sucesso!", "data": data })))
}

pub(super) async fn carta_correcao(State(st): State<FiscalState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let (referencia, correcao) = (txt(&body, "referencia"), txt(&body, "correcao"));
    if referencia.is_empty() || correcao.is_empty() {
        return Err(ApiError::bad_request("Referência da NF-e e texto da correção são obrigatórios."));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API não configurado.")?;
    let data = focus.carta_correcao(&referencia, &correcao).await.map_err(|e| erro_repassado(e, ""))?;
    Ok(Json(json!({ "ok": true, "mensagem": "Carta de Correção registrada com sucesso!", "data": data })))
}

async fn inutilizar(st: FiscalState, body: Value, tipo: TipoNota) -> ApiResult<Json<Value>> {
    let serie = txt(&body, "serie");
    let (ini, fim) = (num(&body, "numeroInicial"), num(&body, "numeroFinal"));
    if serie.trim().is_empty() || ini <= 0.0 || fim <= 0.0 {
        return Err(ApiError::bad_request("Série, número inicial e número final são obrigatórios."));
    }
    let justificativa = txt(&body, "justificativa");
    if justificativa.trim().chars().count() < 15 {
        return Err(ApiError::bad_request(
            "A justificativa de inutilização deve ter no mínimo 15 caracteres (Exigência SEFAZ).",
        ));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API fiscal não configurado.")?;
    if so_digitos(cfg.cnpj.as_deref().unwrap_or("")).is_empty() {
        return Err(ApiError::bad_request("CNPJ da empresa não configurado."));
    }
    let (ini, fim) = (ini as u32, fim as u32);
    let data = focus.inutilizar(modelo(tipo), &serie, ini, fim, &justificativa).await.map_err(|e| erro_repassado(e, ""))?;
    let rotulo = if tipo == TipoNota::Nfce { "Inutilização NFC-e da faixa" } else { "Inutilização da faixa" };
    Ok(Json(json!({
        "ok": true,
        "mensagem": format!("{rotulo} {ini} a {fim} (Série {serie}) homologada com sucesso na SEFAZ!"),
        "data": data,
    })))
}

pub(super) async fn inutilizar_nfe(State(st): State<FiscalState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    inutilizar(st, body, TipoNota::Nfe).await
}

pub(super) async fn inutilizar_nfce(State(st): State<FiscalState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    inutilizar(st, body, TipoNota::Nfce).await
}

pub(super) async fn send_email(State(st): State<FiscalState>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let (referencia, email) = (txt(&body, "referencia"), txt(&body, "email"));
    if referencia.is_empty() || email.is_empty() {
        return Err(ApiError::bad_request("Referência da nota e e-mail são obrigatórios."));
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token fiscal não configurado.")?;
    let tipo = if referencia.starts_with("nfe_") { "nfe" } else { "nfce" };
    let r = focus
        .requisitar(
            Method::POST,
            &format!("/v2/{tipo}/{}/email", uri_seg(&referencia)),
            Some(&json!({ "emails": [email.trim()] })),
        )
        .await
        .map_err(erro_fiscal)?;
    if !r.ok() {
        let msg = opt(&r.corpo, "mensagem").or_else(|| opt(&r.corpo, "codigo")).unwrap_or_else(|| r.corpo.to_string());
        return Err(ApiError::bad_request(format!("Focus NFe: {msg}")));
    }
    Ok(Json(json!({ "success": true, "message": "Nota fiscal enviada por e-mail com sucesso!" })))
}

// ============================================================
// Listas e reimpressão
// ============================================================

#[derive(Deserialize)]
pub(super) struct TipoQuery {
    tipo: Option<String>,
}

fn tipo_da_query(t: &Option<String>) -> Option<TipoNota> {
    match t.as_deref() {
        Some("nfce") => Some(TipoNota::Nfce),
        Some("nfe") => Some(TipoNota::Nfe),
        _ => None,
    }
}

pub(super) async fn recent_notes(State(st): State<FiscalState>, Query(q): Query<TipoQuery>) -> ApiResult<Json<Value>> {
    let tipo = tipo_da_query(&q.tipo);
    let mut lista = notas::emitidas_recentes(&st.db, tipo).await?;
    // com o tipo escolhido, notas ainda processando ou sem número são consultadas na Focus
    if tipo.is_some() {
        if let Some(focus) = st.focus(&st.config().await?) {
            for n in lista.iter_mut().filter(|n| n.precisa_sincronizar()) {
                sincronizar(&st, &focus, n).await;
            }
        }
    }
    Ok(Json(json!(lista)))
}

fn com_prazo(n: &NotaEmitida, extra: Value) -> Value {
    let mut v = json!(n);
    if let (Some(obj), Value::Object(e)) = (v.as_object_mut(), extra) {
        obj.extend(e);
    }
    v
}

pub(super) async fn cancelable_nfce(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    let agora = Utc::now();
    let lista = notas::emitidas_cancelaveis(&st.db, TipoNota::Nfce, agora).await?;
    let out: Vec<Value> = lista
        .iter()
        .map(|(n, restam)| {
            com_prazo(n, json!({ "diffMinutes": n.minutos_desde(agora), "minutesRemaining": restam, "isCancelable": true }))
        })
        .collect();
    Ok(Json(json!(out)))
}

pub(super) async fn cancelable_nfe(State(st): State<FiscalState>) -> ApiResult<Json<Value>> {
    if let Some(focus) = st.focus(&st.config().await?) {
        for mut n in notas::emitidas_para_sincronizar(&st.db, TipoNota::Nfe, 200).await? {
            sincronizar(&st, &focus, &mut n).await;
        }
    }
    let agora = Utc::now();
    let lista = notas::emitidas_cancelaveis(&st.db, TipoNota::Nfe, agora).await?;
    let out: Vec<Value> = lista
        .iter()
        .map(|(n, _)| {
            let diff = n.minutos_desde(agora);
            com_prazo(
                n,
                json!({
                    "diffMinutes": diff,
                    "hoursRemaining": (24 - diff.div_euclid(60)).max(0),
                    "minutesInHour": (60 - diff.rem_euclid(60)).max(0),
                    "isCancelable": true,
                }),
            )
        })
        .collect();
    Ok(Json(json!(out)))
}

fn resposta_reimpressao(headers: &HeaderMap, nota: &NotaEmitida) -> Json<Value> {
    Json(json!({
        "nota": nota,
        "status": nota.status,
        "caminhoDanfe": danfe_url(headers, &nota.referencia),
        "chaveAcesso": nota.chave,
        "success": true,
    }))
}

pub(super) async fn reprint_by_number(
    State(st): State<FiscalState>,
    headers: HeaderMap,
    Path(numero): Path<String>,
) -> ApiResult<Json<Value>> {
    let numero = numero.trim().to_string();
    if numero.is_empty() {
        return Err(ApiError::bad_request("Número da NFC-e não informado."));
    }
    let candidatas = notas::buscar_emitidas_por_numero(&st.db, &numero, None).await?;
    match candidatas.iter().find(|n| n.tipo() == TipoNota::Nfce) {
        Some(n) => Ok(resposta_reimpressao(&headers, n)),
        None if candidatas.iter().any(|n| n.tipo() == TipoNota::Nfe) => Err(ApiError::not_found(format!(
            "A nota Nº {numero} foi emitida como NF-e (DANFE Modelo 55) e não NFC-e. Consulte na tela de Reimprimir NF-e."
        ))),
        None => Err(ApiError::not_found(format!(
            "Nenhuma NFC-e (Modelo 65) localizada no histórico com o número {numero}."
        ))),
    }
}

/// "1", "1/2", "1-2", "1 serie 2" → (número, série).
fn numero_e_serie(texto: &str) -> Option<(String, Option<String>)> {
    let t = texto.trim();
    let fim_num = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    if fim_num == 0 {
        return None;
    }
    let (numero, resto) = t.split_at(fim_num);
    if resto.is_empty() {
        return Some((numero.into(), None));
    }
    let resto = resto.trim_start_matches(['/', '-', ' ', '\t']);
    if resto.len() == t.len() - fim_num {
        return None; // não tinha separador
    }
    let minusculo = resto.to_lowercase();
    let resto = ["série", "serie"]
        .iter()
        .find_map(|p| minusculo.strip_prefix(p).map(|r| &resto[resto.len() - r.len()..]))
        .unwrap_or(resto)
        .trim_start();
    (!resto.is_empty() && resto.chars().all(|c| c.is_ascii_digit())).then(|| (numero.into(), Some(resto.into())))
}

#[derive(Deserialize)]
pub(super) struct SerieQuery {
    serie: Option<String>,
}

pub(super) async fn nfe_reprint(
    State(st): State<FiscalState>,
    headers: HeaderMap,
    Path(busca): Path<String>,
    Query(q): Query<SerieQuery>,
) -> ApiResult<Json<Value>> {
    let busca = busca.trim().to_string();
    if busca.is_empty() {
        return Err(ApiError::bad_request("Número ou Referência da NF-e não informado."));
    }
    let mut serie = q.serie.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let chave_buscada = parse_chave_acesso(&busca).filter(|_| busca.len() == 44 && busca.chars().all(|c| c.is_ascii_digit()));
    let numero = match &chave_buscada {
        Some(k) => {
            serie = Some(k.serie.clone());
            k.numero.clone()
        }
        None => match numero_e_serie(&busca) {
            Some((n, s)) => {
                serie = serie.or(s);
                n
            }
            None => busca.clone(),
        },
    };

    let cfg = st.config().await?;
    let focus = st.focus(&cfg);
    if let Some(focus) = &focus {
        for mut n in notas::emitidas_para_sincronizar(&st.db, TipoNota::Nfe, 15).await? {
            sincronizar(&st, focus, &mut n).await;
        }
    }

    // por referência, chave ou id; depois pelo número (e série)
    let mut candidatas: Vec<NotaEmitida> = Vec::new();
    for alvo in [busca.clone(), format!("nfe_{busca}")] {
        candidatas.extend(notas::buscar_emitida(&st.db, &alvo).await?);
    }
    candidatas.extend(notas::buscar_emitidas_por_numero(&st.db, &numero, None).await?);
    let serie_ok = |n: &NotaEmitida| match (&serie, n.serie.as_deref().map(str::trim)) {
        (Some(s), Some(ns)) if !ns.is_empty() => ns == s,
        _ => true,
    };
    let mut nota = candidatas.iter().find(|n| n.tipo() == TipoNota::Nfe && serie_ok(n)).cloned();

    // a chave de acesso traz o número com 9 dígitos
    if nota.is_none() {
        if let Ok(n) = numero.parse::<u64>() {
            nota = notas::buscar_emitidas_chave_contendo(&st.db, &format!("{n:09}"))
                .await?
                .into_iter()
                .find(|c| {
                    c.tipo() == TipoNota::Nfe
                        && match (&serie, c.chave.as_deref().and_then(parse_chave_acesso)) {
                            (Some(s), Some(k)) => &k.serie == s,
                            _ => true,
                        }
                });
        }
    }

    if let (Some(n), Some(focus)) = (nota.as_mut(), &focus) {
        if n.precisa_sincronizar() || n.chave.is_none() {
            sincronizar(&st, focus, n).await;
        }
    }

    // fora do banco: consulta a Focus pela chave ou referência e grava o que ela confirmar
    if nota.is_none() {
        let alvo = chave_buscada.as_ref().map(|k| k.chave.clone()).or_else(|| busca.starts_with("nfe_").then(|| busca.clone()));
        if let (Some(alvo), Some(focus)) = (alvo, &focus) {
            if let Some(n) = nfe_pela_focus(&st, focus, &headers, &alvo).await? {
                return Ok(resposta_reimpressao(&headers, &n));
            }
        }
    }

    match nota {
        Some(n) => Ok(resposta_reimpressao(&headers, &n)),
        None if candidatas.iter().any(|n| n.tipo() == TipoNota::Nfce) => Err(ApiError::not_found(format!(
            "A nota Nº {busca} foi emitida como NFC-e (Cupom Fiscal Modelo 65) e não NF-e. Consulte na tela de Reimprimir NFC-e."
        ))),
        None => Err(ApiError::not_found(format!("Nenhuma NF-e (Modelo 55) encontrada com o termo informado: {busca}."))),
    }
}

/// Nota que não está no banco: só é gravada se a Focus devolver os dados dela (nada é inventado).
async fn nfe_pela_focus(
    st: &FiscalState,
    focus: &FocusClient,
    headers: &HeaderMap,
    alvo: &str,
) -> ApiResult<Option<NotaEmitida>> {
    let seg = uri_seg(alvo);
    let caminhos = [
        format!("/v2/nfe/{seg}?completa=1"),
        format!("/v2/nfe/{seg}.json"),
        format!("/v2/nfes_recebidas/{seg}.json"),
        format!("/v2/nfe?chave={seg}"),
    ];
    let mut dados = None;
    for c in &caminhos {
        if let Ok(r) = focus.requisitar(Method::GET, c, None).await {
            let d = &r.corpo;
            if r.ok() && (r.status() == "autorizado" || ["chave_nfe", "chave", "numero"].iter().any(|k| d.get(k).is_some())) {
                dados = Some(r.corpo);
                break;
            }
        }
    }
    let Some(d) = dados else {
        return Ok(None);
    };
    let campo = |k: &str| opt(&d, k);
    let referencia = campo("ref").unwrap_or_else(|| format!("nfe_{alvo}"));
    let k = parse_chave_acesso(alvo);
    let danfe = match campo("caminho_danfe") {
        Some(c) if c.starts_with("http") => c,
        Some(c) => format!("{}{c}", focus.base_url()),
        None => danfe_url(headers, alvo),
    };
    let reg = RegistroEmitida {
        referencia: referencia.clone(),
        chave: Some(campo("chave_nfe").or_else(|| campo("chave")).unwrap_or_else(|| alvo.to_string())),
        numero: campo("numero").or_else(|| k.as_ref().map(|k| k.numero.clone())),
        serie: campo("serie").or_else(|| k.as_ref().map(|k| k.serie.clone())),
        data_emissao: campo("data_emissao"),
        valor_total: Some(num(&d, "valor_total")).filter(|v| *v > 0.0).or(Some(num(&d, "total"))),
        status: Some(campo("status").unwrap_or_else(|| "autorizado".into())),
        xml_url: Some(campo("caminho_xml_nota_fiscal").unwrap_or_else(|| format!("{}/v2/nfe/{alvo}.xml", focus.base_url()))),
        pdf_url: Some(danfe),
    };
    Ok(Some(notas::gravar_emitida(&st.db, &reg).await?))
}

pub(super) async fn reprint_ref(
    State(st): State<FiscalState>,
    headers: HeaderMap,
    Path(referencia): Path<String>,
) -> ApiResult<Json<Value>> {
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "Token da API Fiscal não configurado.")?;
    let r = focus.consultar(Modelo::Nfce, &referencia, false).await.map_err(erro_fiscal)?;
    if !r.ok() {
        let msg = opt(&r.corpo, "mensagem")
            .or_else(|| r.corpo.get("erros").map(Value::to_string))
            .unwrap_or_else(|| "Erro ao buscar nota na Focus NFe.".into());
        return Err(ApiError::bad_request(msg));
    }
    if r.status() != "autorizado" {
        return Err(ApiError::bad_request(format!("Nota não está autorizada (Status: {}).", r.status())));
    }
    Ok(Json(json!({
        "success": true,
        "status": "autorizado",
        "chaveAcesso": opt(&r.corpo, "chave_nfe"),
        "caminhoDanfe": danfe_url(&headers, &referencia),
        "xmlUrl": opt(&r.corpo, "caminho_xml_nota_fiscal"),
    })))
}

// ============================================================
// DANFE (proxy da Focus para os APKs e o navegador)
// ============================================================

fn texto(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], msg.into()).into_response()
}

fn pdf(bytes: Vec<u8>, nome: &str) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/pdf".to_string()), (header::CONTENT_DISPOSITION, format!("inline; filename=\"{nome}\""))],
        Body::from(bytes),
    )
        .into_response()
}

pub(super) async fn danfe(State(st): State<FiscalState>, Path(referencia): Path<String>) -> Response {
    match danfe_interno(&st, &referencia).await {
        Ok(r) => r,
        Err(e) => texto(StatusCode::INTERNAL_SERVER_ERROR, format!("Erro interno ao carregar DANFE: {e}")),
    }
}

async fn danfe_interno(st: &FiscalState, referencia: &str) -> Result<Response, String> {
    let cfg = st.config().await.map_err(|e| e.body.to_string())?;
    let Some(focus) = st.focus(&cfg) else {
        return Ok(texto(StatusCode::BAD_REQUEST, "Token da Focus NFe não configurado nas Configurações Fiscais."));
    };
    let tipo = if referencia.starts_with("nfe_") { Modelo::Nfe } else { Modelo::Nfce };
    let r = focus.consultar(tipo, referencia, true).await.map_err(|e| e.to_string())?;

    if !r.ok() {
        // sem a referência na Focus: tenta o PDF oficial pela chave de 44 dígitos
        let chave = if referencia.len() == 44 && referencia.chars().all(|c| c.is_ascii_digit()) {
            Some(referencia.to_string())
        } else {
            notas::buscar_emitida(&st.db, referencia)
                .await
                .map_err(|e| e.to_string())?
                .and_then(|n| n.chave)
                .filter(|c| c.len() == 44)
        };
        if let Some(chave) = chave {
            if let Some(bytes) = focus.baixar_danfe_pdf_por_chave(&chave).await {
                return Ok(pdf(bytes, &format!("DANFE_{chave}.pdf")));
            }
        }
        let msg = opt(&r.corpo, "mensagem")
            .or_else(|| r.corpo.get("erros").map(Value::to_string))
            .unwrap_or_else(|| "Nota não encontrada na SEFAZ.".into());
        let status = StatusCode::from_u16(r.http_status).unwrap_or(StatusCode::BAD_GATEWAY);
        return Ok(texto(status, format!("Erro Focus NFe ({}): {msg}", r.http_status)));
    }

    let Some(caminho) = r.caminho_danfe() else {
        let status = opt(&r.corpo, "status").unwrap_or_else(|| "desconhecido".into());
        return Ok(texto(
            StatusCode::NOT_FOUND,
            format!("O DANFE/Cupom Fiscal ainda não está disponível na SEFAZ. Status atual da nota: {status}."),
        ));
    };
    let url = if caminho.starts_with("http") {
        caminho
    } else {
        format!("{}{}{caminho}", focus.base_url(), if caminho.starts_with('/') { "" } else { "/" })
    };
    let reg = RegistroEmitida {
        referencia: referencia.to_string(),
        pdf_url: Some(url.clone()),
        status: Some(opt(&r.corpo, "status").unwrap_or_else(|| "autorizado".into())),
        ..Default::default()
    };
    if let Err(e) = notas::atualizar_emitida(&st.db, &reg).await {
        tracing::warn!("DANFE: não atualizou a nota {referencia}: {e}");
    }

    let Ok(arquivo) = focus.baixar(&url).await else {
        return Ok(Redirect::temporary(&url).into_response());
    };
    let html = url.to_lowercase().contains(".html") || arquivo.content_type.to_lowercase().contains("text/html");
    if !html {
        return Ok(pdf(arquivo.bytes, &format!("danfe-{referencia}.pdf")));
    }
    let p = importacao::ler_ajustes_impressao_danfe(&st.db).await.map_err(|e| e.to_string())?;
    let ajustes = AjustesDanfe {
        largura_papel: p.paper_width,
        margem_topo: p.margin_top,
        margem_base: p.margin_bottom,
        margem_esquerda: p.margin_left,
        margem_direita: p.margin_right,
        tamanho_qr: p.qr_size,
        escala_fonte: p.font_scale,
        nome_fantasia: cfg.nome_fantasia.clone(),
        razao_social: cfg.razao_social.clone(),
    };
    let html = ajustar_danfe_nfce_html(&String::from_utf8_lossy(&arquivo.bytes), &ajustes);
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response())
}


// ============================================================
// XML autorizado de uma nota emitida
// ============================================================

/// Do cofre de SAÍDA (mês da emissão ou do cadastro) ou, se não estiver lá, baixado da Focus e guardado.
pub(super) async fn xml_emitida(State(st): State<FiscalState>, Path(referencia): Path<String>) -> ApiResult<Response> {
    let Some(nota) = notas::buscar_emitida(&st.db, &referencia).await? else {
        return Err(ApiError::not_found("Nota não encontrada no histórico."));
    };
    let nome = nota.chave.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| nota.referencia.clone());
    let mut meses = vec![(nota.created_at.0.year(), nota.created_at.0.month())];
    if let Some(d) = nota.data_emissao.as_deref().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()) {
        meses.insert(0, (d.year(), d.month()));
    }
    for (ano, mes) in meses {
        if let Some(p) = notas::caminho_cofre(&st.cofre(), notas::LadoCofre::Saida, &nome, ano, mes) {
            if let Ok(xml) = std::fs::read_to_string(&p) {
                return Ok(([(header::CONTENT_TYPE, "application/xml")], xml).into_response());
            }
        }
    }
    let cfg = st.config().await?;
    let focus = st.focus_ou(&cfg, "XML não está no cofre e o Token da Focus NFe não está configurado.")?;
    let r = focus.consultar(modelo(nota.tipo()), &nota.referencia, true).await.map_err(erro_fiscal)?;
    let caminho = opt(&r.corpo, "caminho_xml_nota_fiscal").or(nota.xml_url.clone());
    let Some(caminho) = caminho else {
        return Err(ApiError::not_found("A Focus NFe ainda não disponibilizou o XML desta nota."));
    };
    let arquivo = focus.baixar(&caminho).await.map_err(erro_fiscal)?;
    let xml = String::from_utf8_lossy(&arquivo.bytes).into_owned();
    if !(xml.contains("<nfeProc") || xml.contains("<NFe")) {
        return Err(ApiError::not_found("A Focus NFe ainda não disponibilizou o XML desta nota."));
    }
    if let Err(e) = notas::arquivar_xml(&st.cofre(), notas::LadoCofre::Saida, &nome, &xml) {
        tracing::warn!("cofre de XML: não gravou a nota {nome}: {e}");
    }
    Ok(([(header::CONTENT_TYPE, "application/xml")], xml).into_response())
}

#[cfg(test)]
mod testes_unidade {
    use super::numero_e_serie;

    #[test]
    fn numero_com_serie() {
        let ns = |t: &str| numero_e_serie(t).map(|(n, s)| (n, s.unwrap_or_default()));
        assert_eq!(ns("12"), Some(("12".into(), String::new())));
        assert_eq!(ns("12/2"), Some(("12".into(), "2".into())));
        assert_eq!(ns("12 - 3"), Some(("12".into(), "3".into())));
        assert_eq!(ns("12 série 2"), Some(("12".into(), "2".into())));
        assert_eq!(ns("12 SERIE2"), Some(("12".into(), "2".into())));
        assert_eq!(ns("12x"), None);
        assert_eq!(ns("nfe_1"), None);
    }
}
