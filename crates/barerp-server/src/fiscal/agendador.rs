//! Captura automática das NF-e emitidas contra o CNPJ do bar (`nfeRecebidasScheduler.ts`): a primeira
//! 10 s depois de subir o servidor e depois a cada 15 minutos.

use std::time::Duration;

use barerp_db::notas::{self, LadoCofre, RegistroRecebida};
use barerp_fiscal::{ler_nfe_xml, parse_chave_acesso};

use super::FiscalState;
use crate::error::ApiResult;

const PRIMEIRA: Duration = Duration::from_secs(10);
const INTERVALO: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ResultadoSync {
    pub adicionadas: u32,
    pub com_xml: u32,
}

/// Dispara o agendador em segundo plano (uma vez, ao subir o servidor).
///
/// Em build de desenvolvimento só roda com `BARERP_FISCAL_AGENDADOR=1`: testar o app com uma cópia do
/// `bar.db` do cliente não pode consultar a Focus de produção nem manifestar ciência das notas dele.
pub fn iniciar_agendador(state: FiscalState) {
    if cfg!(debug_assertions) && std::env::var("BARERP_FISCAL_AGENDADOR").as_deref() != Ok("1") {
        tracing::info!("captura automática de NF-e recebidas desligada no build de desenvolvimento (BARERP_FISCAL_AGENDADOR=1 liga)");
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(PRIMEIRA).await;
        loop {
            match sincronizar_recebidas(&state).await {
                Ok(r) if r.adicionadas > 0 || r.com_xml > 0 => tracing::info!(
                    "notas recebidas: +{} novas, {} XMLs baixados da SEFAZ",
                    r.adicionadas,
                    r.com_xml
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!("sincronização das notas recebidas: {}", e.body),
            }
            tokio::time::sleep(INTERVALO).await;
        }
    });
    tracing::info!("captura automática de NF-e recebidas ativa (a cada 15 min)");
}

fn arquivar(state: &FiscalState, chave: &str, xml: &str) {
    if let Err(e) = notas::arquivar_xml(&state.cofre(), LadoCofre::Entrada, chave, xml) {
        tracing::warn!("cofre de XML: não gravou a nota de entrada {chave}: {e}");
    }
}

/// Uma rodada: lista as notas da Focus, cadastra as novas (com XML quando a SEFAZ já liberou e ciência
/// automática), baixa o XML das que ainda não tinham e corrige valor zerado. Sem token, não faz nada.
pub(crate) async fn sincronizar_recebidas(state: &FiscalState) -> ApiResult<ResultadoSync> {
    let _uma_por_vez = state.sync.lock().await;
    let mut r = ResultadoSync::default();
    let cfg = state.config().await?;
    let Some(focus) = state.focus(&cfg) else {
        return Ok(r);
    };
    let lista = match focus.listar_nfes_recebidas().await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("Focus NFe: falha ao listar notas recebidas: {e}");
            return Ok(r);
        }
    };
    if let Some(aviso) = &lista.aviso {
        tracing::warn!("{aviso}");
    }

    for item in &lista.notas {
        let chave = &item.chave;
        let k = parse_chave_acesso(chave);
        let base = RegistroRecebida {
            chave: chave.clone(),
            emitente: Some(item.nome_emitente.clone().unwrap_or_else(|| match &k {
                Some(k) => format!("Fornecedor CNPJ {}", k.cnpj_formatado),
                None => "Fornecedor".into(),
            })),
            cnpj_emitente: item.documento_emitente.clone().or_else(|| k.as_ref().map(|k| k.cnpj_emitente.clone())),
            numero: item.numero.clone().or_else(|| k.as_ref().map(|k| k.numero.clone())),
            serie: item.serie.clone().or_else(|| k.as_ref().map(|k| k.serie.clone())),
            data_emissao: item.data_emissao.clone(),
            valor_total: Some(item.valor_total),
            xml_content: None,
        };
        let existente = notas::buscar_recebida(&state.db, chave).await?;

        match existente {
            None => {
                let mut reg = base;
                if let Some(xml) = focus.baixar_xml_recebida(chave).await.and_then(|x| x.xml) {
                    if let Ok(n) = ler_nfe_xml(&xml) {
                        completar_pelo_xml(&mut reg, &n);
                    }
                    arquivar(state, chave, &xml);
                    reg.xml_content = Some(xml);
                    r.com_xml += 1;
                }
                if reg.data_emissao.is_none() {
                    reg.data_emissao = Some(super::agora_iso());
                }
                // nova nota: "pendente" aguardando o 1º bip, ou "finalizada" se já entrou no estoque
                notas::gravar_recebida(&state.db, &reg, None).await?;
                r.adicionadas += 1;
                if item.manifestacao_destinatario.is_none() {
                    if let Err(e) = focus.manifestar_ciencia(chave).await {
                        tracing::warn!("ciência automática da nota {chave}: {e}");
                    }
                }
            }
            Some(atual) if atual.tem_xml() => {
                // nota antiga com valor zerado: corrige pelo vNF do XML ou pelo valor da Focus
                if atual.valor_total.unwrap_or(0.0) <= 0.0 {
                    let valor = notas::vnf_do_xml(atual.xml_content.as_deref().unwrap_or("")).unwrap_or(item.valor_total);
                    if valor > 0.0 {
                        let reg = RegistroRecebida { chave: chave.clone(), valor_total: Some(valor), ..Default::default() };
                        notas::gravar_recebida(&state.db, &reg, None).await?;
                    }
                }
            }
            Some(_) => {
                // ainda sem o XML completo: a SEFAZ pode ter liberado agora
                if let Some(xml) = focus.baixar_xml_recebida(chave).await.and_then(|x| x.xml) {
                    let mut reg = RegistroRecebida { data_emissao: None, ..base };
                    if let Ok(n) = ler_nfe_xml(&xml) {
                        completar_pelo_xml(&mut reg, &n);
                    }
                    reg.xml_content = Some(xml.clone());
                    notas::gravar_recebida(&state.db, &reg, None).await?;
                    arquivar(state, chave, &xml);
                    r.com_xml += 1;
                }
            }
        }
    }
    Ok(r)
}

fn completar_pelo_xml(reg: &mut RegistroRecebida, n: &barerp_fiscal::NfeXml) {
    if !n.emitente.nome.is_empty() && n.emitente.nome != "Fornecedor sem nome" {
        reg.emitente = Some(n.emitente.nome.clone());
    }
    if !n.emitente.documento.is_empty() {
        reg.cnpj_emitente = Some(n.emitente.documento.clone());
    }
    if !n.numero.is_empty() {
        reg.numero = Some(n.numero.clone());
    }
    if !n.serie.is_empty() {
        reg.serie = Some(n.serie.clone());
    }
    if n.data_emissao.is_some() {
        reg.data_emissao = n.data_emissao.clone();
    }
    if n.valor_total > 0.0 {
        reg.valor_total = Some(n.valor_total);
    }
}
