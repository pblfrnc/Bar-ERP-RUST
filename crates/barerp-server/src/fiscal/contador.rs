//! Painel do contador: resumo do mês e o fechamento em `.zip` (XMLs de saída e entrada + relatório CSV).

use std::io::{Cursor, Write};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use barerp_db::importacao::{self, PosicaoEstoque};
use barerp_db::notas::{self, LadoCofre, NotaEmitida, NotaRecebida, TipoNota};
use barerp_fiscal::parece_xml_nfe;
use chrono::Local;
use serde::Deserialize;
use serde_json::{json, Value};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::FiscalState;
use crate::error::{ApiError, ApiResult};

#[derive(Deserialize)]
pub(super) struct MesQuery {
    month: Option<String>,
    #[serde(rename = "includeStock")]
    include_stock: Option<String>,
}

/// "AAAA-MM" (padrão: mês atual em UTC, como no Node).
fn mes(q: &Option<String>) -> ApiResult<(String, i32, u32)> {
    let texto = q.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| chrono::Utc::now().format("%Y-%m").to_string());
    let invalido = || ApiError::bad_request("Mês de referência inválido. Use o formato AAAA-MM.");
    let (a, m) = texto.split_once('-').ok_or_else(invalido)?;
    let ano: i32 = a.trim().parse().map_err(|_| invalido())?;
    let num: u32 = m.trim().parse().map_err(|_| invalido())?;
    if !(1..=12).contains(&num) {
        return Err(invalido());
    }
    Ok((texto, ano, num))
}

/// `Number(v.toFixed(2))`.
fn r2(v: f64) -> f64 {
    format!("{v:.2}").parse().unwrap_or(0.0)
}

/// Valor em reais com vírgula ("12,50").
fn brl(v: f64) -> String {
    format!("{v:.2}").replace('.', ",")
}

fn categorias_json(p: &PosicaoEstoque) -> Vec<Value> {
    p.categorias
        .iter()
        .map(|c| {
            let lucro = c.venda - c.custo;
            let margem = if c.venda > 0.0 { (lucro / c.venda * 1000.0).round() / 10.0 } else { 0.0 };
            json!({
                "name": c.nome, "category": c.nome, "itemsCount": c.itens,
                "cost": r2(c.custo), "totalCost": r2(c.custo),
                "sale": r2(c.venda), "totalSale": r2(c.venda),
                "profit": r2(lucro), "expectedProfit": r2(lucro),
                "margin": margem, "marginPercent": margem,
            })
        })
        .collect()
}

pub(super) async fn month_summary(State(st): State<FiscalState>, Query(q): Query<MesQuery>) -> ApiResult<Json<Value>> {
    let (texto, ano, num) = mes(&q.month)?;
    let r = notas::resumo_fiscal_mes(&st.db, ano, num)
        .await?
        .ok_or_else(|| ApiError::bad_request("Mês de referência inválido. Use o formato AAAA-MM."))?;
    let estoque = importacao::posicao_estoque(&st.db).await?;
    Ok(Json(json!({
        "month": texto,
        "emitidasCount": r.emitidas_count,
        "emitidasTotal": r.emitidas_total,
        "nfceCount": r.nfce_count,
        "nfceTotal": r.nfce_total,
        "nfeCount": r.nfe_count,
        "nfeTotal": r.nfe_total,
        "recebidasCount": r.recebidas_count,
        "recebidasTotal": r.recebidas_total,
        "stockCostTotal": r2(estoque.custo_total),
        "stockSaleTotal": r2(estoque.venda_total),
        "stockProfitTotal": r2(estoque.venda_total - estoque.custo_total),
        "categoriesStock": categorias_json(&estoque),
        "hasNotes": r.has_notes,
    })))
}

fn data_nota(data: &Option<String>, criada: &barerp_db::DbTime) -> String {
    data.clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| criada.0.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// XML de uma nota emitida: do cofre do mês ou baixado da Focus. `None` se não houver o XML verdadeiro.
async fn xml_emitida(st: &FiscalState, focus: Option<&barerp_fiscal::FocusClient>, n: &NotaEmitida, ano: i32, mes: u32) -> Option<Vec<u8>> {
    let nome = n.chave.clone().unwrap_or_else(|| n.referencia.clone());
    if let Some(p) = notas::caminho_cofre(&st.cofre(), LadoCofre::Saida, &nome, ano, mes) {
        if let Ok(b) = std::fs::read(&p) {
            return Some(b);
        }
    }
    let (focus, url) = (focus?, n.xml_url.as_deref()?);
    let a = focus.baixar(url).await.ok()?;
    parece_xml_nfe(&String::from_utf8_lossy(&a.bytes)).then_some(a.bytes)
}

fn xml_recebida(st: &FiscalState, n: &NotaRecebida, ano: i32, mes: u32) -> Option<Vec<u8>> {
    if let Some(p) = notas::caminho_cofre(&st.cofre(), LadoCofre::Entrada, &n.chave, ano, mes) {
        if let Ok(b) = std::fs::read(&p) {
            return Some(b);
        }
    }
    n.xml_content.as_deref().filter(|x| !x.trim_start().starts_with('{') && parece_xml_nfe(x)).map(|x| x.as_bytes().to_vec())
}

fn csv_emitidas(titulo: &str, lista: &[&NotaEmitida]) -> String {
    let mut s = format!("--- {titulo} ---\nNúmero;Série;Data/Hora;Chave de Acesso;Status;Valor (R$)\n");
    for n in lista {
        s += &format!(
            "\"{}\";\"{}\";\"{}\";\"{}\";\"{}\";\"{}\"\n",
            n.numero.clone().unwrap_or_default(),
            n.serie.clone().unwrap_or_default(),
            data_nota(&n.data_emissao, &n.created_at),
            n.chave.clone().unwrap_or_else(|| n.referencia.clone()),
            n.status,
            brl(n.valor_total.unwrap_or(0.0)),
        );
    }
    s + "\n"
}

/// Fechamento do mês para a contabilidade. Nota sem o XML verdadeiro (nem no cofre nem na Focus) não ganha
/// XML inventado: fica listada em `XMLS_FALTANDO.txt` e continua no relatório.
pub(super) async fn export_month(State(st): State<FiscalState>, Query(q): Query<MesQuery>) -> ApiResult<Response> {
    let (texto, ano, num) = mes(&q.month)?;
    let invalido = || ApiError::bad_request("Mês de referência inválido. Use o formato AAAA-MM.");
    let (emitidas, recebidas) = notas::notas_do_mes(&st.db, ano, num).await?.ok_or_else(invalido)?;
    let cfg = st.config().await?;
    let focus = st.focus(&cfg);

    let erro_zip = |e: zip::result::ZipError| ApiError::internal(format!("Erro ao gerar o pacote ZIP: {e}"));
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opcoes = SimpleFileOptions::default();
    let mut faltando: Vec<String> = Vec::new();

    let nfce: Vec<&NotaEmitida> = emitidas.iter().filter(|n| n.tipo() == TipoNota::Nfce).collect();
    let nfe: Vec<&NotaEmitida> = emitidas.iter().filter(|n| n.tipo() == TipoNota::Nfe).collect();
    for (lista, pasta, prefixo) in [(&nfce, "NFCe_Emitidas", "NFCe"), (&nfe, "NFe_Emitidas", "NFe")] {
        for n in lista.iter() {
            let nome = format!(
                "{pasta}/{prefixo}_{}_{}.xml",
                n.numero.clone().unwrap_or_else(|| n.referencia.clone()),
                n.chave.clone().unwrap_or_else(|| "sem_chave".into())
            );
            match xml_emitida(&st, focus.as_ref(), n, ano, num).await {
                Some(b) => {
                    zip.start_file(nome.as_str(), opcoes).map_err(erro_zip)?;
                    zip.write_all(&b).map_err(|e| ApiError::internal(e.to_string()))?;
                }
                None => faltando.push(format!("{nome} (referência {}, status {})", n.referencia, n.status)),
            }
        }
    }
    let recebidas_valor: Vec<(&NotaRecebida, f64)> = recebidas.iter().map(|n| (n, n.valor())).collect();
    for (n, _) in &recebidas_valor {
        let nome = format!("NFe_Recebidas/NFe_{}_{}.xml", n.numero.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "compra".into()), n.chave);
        match xml_recebida(&st, n, ano, num) {
            Some(b) => {
                zip.start_file(nome.as_str(), opcoes).map_err(erro_zip)?;
                zip.write_all(&b).map_err(|e| ApiError::internal(e.to_string()))?;
            }
            None => faltando.push(format!("{nome} (fornecedor {}, status {})", n.emitente.clone().unwrap_or_default(), n.status)),
        }
    }

    // relatório consolidado (separador ponto e vírgula para o Excel)
    let soma = |l: &[&NotaEmitida]| l.iter().map(|n| n.valor_total.unwrap_or(0.0)).sum::<f64>();
    let total_entradas: f64 = recebidas_valor.iter().map(|(_, v)| v).sum();
    let mut csv = format!(
        "RELATÓRIO DE FECHAMENTO FISCAL MENSAL\nMês de Competência:;{texto}\nCNPJ da Empresa:;{}\nData de Emissão do Relatório:;{}\n\n",
        cfg.cnpj.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "Não cadastrado".into()),
        Local::now().format("%d/%m/%Y, %H:%M:%S"),
    );
    csv += "--- RESUMO GERAL ---\n";
    csv += &format!("Total de NFC-e Emitidas (Modelo 65):;{};R$ {}\n", nfce.len(), brl(soma(&nfce)));
    csv += &format!("Total de NF-e Emitidas (Modelo 55):;{};R$ {}\n", nfe.len(), brl(soma(&nfe)));
    csv += &format!("Total de NF-e Recebidas (Entradas/Compras):;{};R$ {}\n\n", recebidas.len(), brl(total_entradas));
    csv += &csv_emitidas("NOTAS FISCAIS DO CONSUMIDOR (NFC-E - MOD. 65)", &nfce);
    csv += &csv_emitidas("NOTAS FISCAIS ELETRÔNICAS (NF-E - MOD. 55)", &nfe);
    csv += "--- NOTAS FISCAIS DE ENTRADA (COMPRAS / FORNECEDORES) ---\nNúmero;Série;Fornecedor;CNPJ Fornecedor;Data Emissão;Status;Valor (R$)\n";
    for (n, v) in &recebidas_valor {
        csv += &format!(
            "\"{}\";\"{}\";\"{}\";\"{}\";\"{}\";\"{}\";\"{}\"\n",
            n.numero.clone().unwrap_or_default(),
            n.serie.clone().unwrap_or_default(),
            n.emitente.clone().unwrap_or_default(),
            n.cnpj_emitente.clone().unwrap_or_default(),
            data_nota(&n.data_emissao, &n.created_at),
            n.status,
            brl(*v),
        );
    }
    csv += "\n--- POSIÇÃO DO ESTOQUE E LUCRO PREVISTO (SPED FISCAL / INVENTÁRIO) ---\n";
    if q.include_stock.as_deref() == Some("true") {
        let p = importacao::posicao_estoque(&st.db).await?;
        let lucro = p.venda_total - p.custo_total;
        let margem = if p.venda_total > 0.0 { lucro / p.venda_total * 100.0 } else { 0.0 };
        csv += &format!("Valor Total do Estoque a Preço de Custo:;R$ {}\n", brl(p.custo_total));
        csv += &format!("Valor Total do Estoque a Preço de Venda:;R$ {}\n", brl(p.venda_total));
        csv += &format!("Lucro Previsto Total do Estoque:;R$ {}\n", brl(lucro));
        csv += &format!("Margem Média Prevista do Estoque:;{}%\n\n", format!("{margem:.1}").replace('.', ","));
        csv += "--- LUCRO PREVISTO POR CATEGORIA DE PRODUTOS ---\nCategoria;Estoque a Custo (R$);Estoque a Venda (R$);Lucro Previsto (R$);Margem Média (%)\n";
        for c in &p.categorias {
            let l = c.venda - c.custo;
            let m = if c.venda > 0.0 { l / c.venda * 100.0 } else { 0.0 };
            csv += &format!(
                "\"{}\";\"{}\";\"{}\";\"{}\";\"{}%\"\n",
                c.nome,
                brl(c.custo),
                brl(c.venda),
                brl(l),
                format!("{m:.1}").replace('.', ",")
            );
        }
        csv += "\n";
    } else {
        csv += "Posição de estoque desabilitada no fechamento deste mês (Aguardando conferência e correção física do inventário).\n\n";
    }
    zip.start_file(format!("Relatorio_Fiscal_{texto}.csv"), opcoes).map_err(erro_zip)?;
    zip.write_all(format!("\u{FEFF}{csv}").as_bytes()).map_err(|e| ApiError::internal(e.to_string()))?;

    if !faltando.is_empty() {
        let aviso = format!(
            "Estas notas estão no relatório, mas o XML autorizado não foi encontrado no cofre do computador nem na Focus NFe.\n\
             Baixe-os no painel da Focus NFe ou peça ao fornecedor antes de enviar à contabilidade.\n\n{}\n",
            faltando.join("\n")
        );
        zip.start_file("XMLS_FALTANDO.txt", opcoes).map_err(erro_zip)?;
        zip.write_all(aviso.as_bytes()).map_err(|e| ApiError::internal(e.to_string()))?;
    }
    let bytes = zip.finish().map_err(erro_zip)?.into_inner();
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"Fechamento_Contabil_{texto}.zip\"")),
        ],
        Body::from(bytes),
    )
        .into_response())
}
