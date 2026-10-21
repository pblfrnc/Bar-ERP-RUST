//! Entrada de mercadoria pela NF-e de compra e consultas de produto usadas pelas telas fiscais.
//!
//! Tradução de `/apply-import` e dos `supplier.upsert` de `server/src/routes/fiscal.ts`, de
//! `getNextSequentialCode` (`server/src/services/catalogService.ts`), da posição de estoque do
//! `/month-summary` e de `getPrinterSettingsSafe` (`server/src/routes/settings.ts`).

use barerp_core::ids;
use barerp_core::pricing::round2;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqliteConnection};

use crate::{Db, DbError, DbResult, DbTime};

// ============================================================
// Fornecedor da nota
// ============================================================

/// Emitente da NF-e. Campos `None` não apagam o que já está no cadastro.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FornecedorNfe {
    /// CNPJ ou CPF (com ou sem pontuação).
    pub documento: String,
    pub nome: String,
    pub fantasia: Option<String>,
    pub ie: Option<String>,
    pub telefone: Option<String>,
    pub endereco: Option<String>,
    pub cidade: Option<String>,
    pub uf: Option<String>,
}

/// Cria ou atualiza o fornecedor pelo documento e devolve o id. `None` se a nota não tiver documento.
pub async fn upsert_fornecedor_nfe(db: &Db, f: &FornecedorNfe) -> DbResult<Option<String>> {
    let documento: String = f.documento.chars().filter(|c| c.is_ascii_digit()).collect();
    if documento.is_empty() {
        return Ok(None);
    }
    let nome = if f.nome.trim().is_empty() { "Fornecedor sem nome" } else { f.nome.trim() };
    let agora = DbTime::now();
    let mut tx = db.begin_write().await?;
    let existente: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Supplier" WHERE "document" = ?"#)
        .bind(&documento)
        .fetch_optional(&mut **tx)
        .await?;
    let id = match existente {
        Some(id) => {
            sqlx::query(
                r#"UPDATE "Supplier" SET "name" = ?, "tradeName" = COALESCE(?, "tradeName"), "ie" = COALESCE(?, "ie"),
                     "phone" = COALESCE(?, "phone"), "address" = COALESCE(?, "address"), "city" = COALESCE(?, "city"),
                     "state" = COALESCE(?, "state"), "updatedAt" = ?
                   WHERE "id" = ?"#,
            )
            .bind(nome)
            .bind(&f.fantasia)
            .bind(&f.ie)
            .bind(&f.telefone)
            .bind(&f.endereco)
            .bind(&f.cidade)
            .bind(&f.uf)
            .bind(agora)
            .bind(&id)
            .execute(&mut **tx)
            .await?;
            id
        }
        None => {
            let id = ids::cuid();
            sqlx::query(
                r#"INSERT INTO "Supplier" ("id", "name", "tradeName", "document", "ie", "phone", "address", "city", "state",
                                           "createdAt", "updatedAt")
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(nome)
            .bind(&f.fantasia)
            .bind(&documento)
            .bind(&f.ie)
            .bind(&f.telefone)
            .bind(&f.endereco)
            .bind(&f.cidade)
            .bind(&f.uf)
            .bind(agora)
            .bind(agora)
            .execute(&mut **tx)
            .await?;
            id
        }
    };
    tx.commit().await?;
    Ok(Some(id))
}

// ============================================================
// Código sequencial do produto
// ============================================================

/// Próximo código livre da categoria (`getNextSequentialCode`): começa no `codeStart` da categoria (ou numa
/// faixa pelo nome: bebidas 5001, doces/tabaco 6001, cozinha 1001) e pula qualquer código já usado.
pub async fn proximo_codigo_categoria(conn: &mut SqliteConnection, category_id: &str) -> DbResult<String> {
    let cat: Option<(String, i64, Option<i64>)> =
        sqlx::query_as(r#"SELECT "name", "sortOrder", "codeStart" FROM "Category" WHERE "id" = ?"#)
            .bind(category_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((nome, sort_order, code_start)) = cat else {
        return Ok("1001".into());
    };
    let base = match code_start.filter(|c| *c >= 1) {
        Some(c) => c,
        None => {
            let n = nome.to_lowercase();
            let tem = |palavras: &[&str]| palavras.iter().any(|p| n.contains(p));
            if tem(&["bebida", "cerveja", "chope", "chopp", "drink", "dose", "destilado", "alco"]) {
                5001
            } else if tem(&["trident", "chiclete", "bala", "doce", "sobremesa", "tabaco", "cigarro"]) {
                6001
            } else if tem(&[
                "cozinha", "petisco", "porcao", "porção", "lanche", "burger", "prato", "alimento", "comida", "refeic",
                "almoco",
            ]) {
                1001
            } else {
                // Como no Node: `(sortOrder || 1) > 0 ? sortOrder : 1`, então sortOrder 0 dá base 1
                (if sort_order >= 0 { sort_order } else { 1 }) * 1000 + 1
            }
        }
    };
    let usados: Vec<Option<String>> = sqlx::query_scalar(r#"SELECT "code" FROM "Product""#).fetch_all(&mut *conn).await?;
    let usados: std::collections::HashSet<String> = usados.into_iter().flatten().map(|c| c.trim().to_string()).collect();
    let mut candidato = base;
    while usados.contains(&candidato.to_string()) {
        candidato += 1;
    }
    Ok(candidato.to_string())
}

// ============================================================
// Entrada da nota no estoque (/apply-import)
// ============================================================

/// Número que pode vir como número ou texto no JSON da tela.
fn num_flex<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().replace(',', ".").parse().ok(),
        _ => None,
    })
}

/// Item lido do XML, como a tela devolve (`xmlItem`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemXmlImportado {
    pub name: String,
    #[serde(deserialize_with = "num_flex")]
    pub quantity: Option<f64>,
    #[serde(deserialize_with = "num_flex")]
    pub unit_cost: Option<f64>,
    pub ncm: Option<String>,
    pub cfop: Option<String>,
    pub ean: Option<String>,
    pub code: Option<String>,
    pub unit: Option<String>,
}

/// Um item da conferência: vincular a um produto (`LINK`) ou cadastrar (`NEW`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemImportacao {
    pub xml_item: ItemXmlImportado,
    pub action: String,
    pub product_id: Option<String>,
    pub category_id: Option<String>,
    pub supplier_id: Option<String>,
    pub supplier_name: Option<String>,
    /// Unidades por item da nota (ex.: 12 latas por caixa). Ausente ou inválido = 1.
    #[serde(deserialize_with = "num_flex")]
    pub conversion_factor: Option<f64>,
    #[serde(deserialize_with = "num_flex")]
    pub new_sale_price: Option<f64>,
    pub update_box_price: bool,
    #[serde(deserialize_with = "num_flex")]
    pub new_box_price: Option<f64>,
    #[serde(deserialize_with = "num_flex")]
    pub target_margin: Option<f64>,
}

/// Corpo do `/apply-import` (a forma antiga, só o array de itens, entra como `itens` com o resto vazio).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportacaoXml {
    pub items: Vec<ItemImportacao>,
    pub chave_acesso: Option<String>,
    pub supplier_id: Option<String>,
    pub vendor_name: Option<String>,
    #[serde(deserialize_with = "num_flex")]
    pub valor_total: Option<f64>,
    #[serde(deserialize_with = "num_flex_texto")]
    pub numero: Option<String>,
    #[serde(deserialize_with = "num_flex_texto")]
    pub serie: Option<String>,
    pub data_emissao: Option<String>,
    pub user_name: Option<String>,
}

fn num_flex_texto<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) if !s.is_empty() => Some(s),
        _ => None,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ResultadoImportacao {
    pub updated: u32,
    pub created: u32,
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn texto_ou_nulo(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty())
}

#[allow(clippy::too_many_arguments)]
async fn gravar_historico(
    conn: &mut SqliteConnection,
    product_id: &str,
    old_cost: Option<f64>,
    new_cost: f64,
    old_price: f64,
    new_price: f64,
    usuario: &str,
    fator: f64,
    chave: Option<&str>,
    inicial: bool,
) -> DbResult<()> {
    let (cost_diff, cost_percent) = if inicial {
        (None, None)
    } else {
        (
            Some(old_cost.map_or(new_cost, |o| round2(new_cost - o))),
            old_cost.filter(|o| *o > 0.0).map(|o| round2((new_cost - o) / o * 100.0)),
        )
    };
    let price_diff = round2(new_price - old_price);
    let price_percent = if old_price > 0.0 { round2((new_price - old_price) / old_price * 100.0) } else { 0.0 };
    let motivo = if fator > 1.0 { format!("IMPORT_XML (Fator x{fator})") } else { "IMPORT_XML".into() };
    sqlx::query(
        r#"INSERT INTO "PriceHistory" ("id", "productId", "oldCostPrice", "newCostPrice", "oldPrice", "newPrice", "costDiff",
                                       "costPercent", "priceDiff", "pricePercent", "changedBy", "reason", "nfeChave", "createdAt")
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(ids::cuid())
    .bind(product_id)
    .bind(old_cost)
    .bind(new_cost)
    .bind(old_price)
    .bind(new_price)
    .bind(cost_diff)
    .bind(cost_percent)
    .bind(price_diff)
    .bind(price_percent)
    .bind(usuario)
    .bind(motivo)
    .bind(chave)
    .bind(DbTime::now())
    .execute(conn)
    .await?;
    Ok(())
}

fn preco_pela_margem(custo: f64, margem: f64) -> f64 {
    if margem < 100.0 {
        round2(custo / (1.0 - margem / 100.0))
    } else {
        round2(custo * 2.0)
    }
}

/// Dá entrada da nota: soma o estoque (quantidade × fator), atualiza custo e preço (pela margem do produto
/// quando a tela não mandar o preço), preço/custo de caixa, dados fiscais e fornecedor; cadastra os
/// produtos novos com código sequencial; grava o histórico de preços com a chave da nota e marca a
/// `NotaRecebida` como finalizada. Tudo numa transação: ou entra a nota inteira, ou nada.
pub async fn aplicar_importacao_xml(db: &Db, imp: &ImportacaoXml, usuario: Option<&str>) -> DbResult<ResultadoImportacao> {
    let usuario = usuario.or(imp.user_name.as_deref()).filter(|u| !u.is_empty()).unwrap_or("Operador");
    let chave: Option<String> = imp
        .chave_acesso
        .as_deref()
        .map(|c| c.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
        .filter(|c| !c.is_empty());
    let agora = DbTime::now();
    let mut res = ResultadoImportacao::default();
    let mut tx = db.begin_write().await?;

    for item in &imp.items {
        let fornecedor_id = texto_ou_nulo(&item.supplier_id).or(texto_ou_nulo(&imp.supplier_id));
        let fornecedor_nome = texto_ou_nulo(&item.supplier_name).or(texto_ou_nulo(&imp.vendor_name));
        let fator = item.conversion_factor.filter(|f| *f > 0.0).unwrap_or(1.0);
        let qtd_nota = item.xml_item.quantity.unwrap_or(0.0);
        let custo_nota = item.xml_item.unit_cost.unwrap_or(0.0);
        let qtd = (qtd_nota * fator).round() as i64;
        let custo = round4(custo_nota / fator);
        let x = &item.xml_item;

        match (item.action.as_str(), texto_ou_nulo(&item.product_id), texto_ou_nulo(&item.category_id)) {
            ("LINK", Some(product_id), _) => {
                #[derive(FromRow)]
                struct ProductPricingInfo {
                    #[sqlx(rename = "costPrice")]
                    cost_price: Option<f64>,
                    price: f64,
                    #[sqlx(rename = "targetMargin")]
                    target_margin: Option<f64>,
                    #[sqlx(rename = "hasBoxPrice")]
                    has_box_price: bool,
                    #[sqlx(rename = "boxQuantity")]
                    box_quantity: Option<f64>,
                    #[sqlx(rename = "boxPrice")]
                    box_price: Option<f64>,
                }

                let atual: Option<ProductPricingInfo> = sqlx::query_as(
                    r#"SELECT "costPrice", "price", "targetMargin", "hasBoxPrice", CAST("boxQuantity" AS REAL) AS "boxQuantity", "boxPrice"
                       FROM "Product" WHERE "id" = ?"#,
                )
                .bind(product_id)
                .fetch_optional(&mut **tx)
                .await?;
                let Some(atual) = atual else {
                    return Err(DbError::not_found(format!("Produto {product_id} não encontrado para a entrada da nota.")));
                };
                let (custo_antigo, preco_antigo, margem, tem_caixa, qtd_caixa, preco_caixa) = (
                    atual.cost_price,
                    atual.price,
                    atual.target_margin,
                    atual.has_box_price,
                    atual.box_quantity,
                    atual.box_price,
                );
                let preco = match (item.new_sale_price, margem) {
                    (Some(p), _) if p.is_finite() => p,
                    (_, Some(m)) if m > 0.0 && custo > 0.0 => preco_pela_margem(custo, m),
                    _ => preco_antigo,
                };

                // preço e custo da caixa
                let mut caixa: Option<(f64, f64, Option<f64>)> = None;
                if let (true, true, Some(qc)) = (item.update_box_price, tem_caixa, qtd_caixa.filter(|q| *q != 0.0)) {
                    let unidades = if fator > 1.0 { fator } else { qc };
                    let novo_preco_caixa = match (item.new_box_price, preco_caixa) {
                        (Some(p), _) => p,
                        (None, Some(pc)) if preco_antigo > 0.0 && pc != 0.0 => round2(pc * (preco / preco_antigo)),
                        _ => round2(preco * unidades),
                    };
                    let custo_caixa = if custo_nota > 0.0 { custo_nota } else { round2(custo * unidades) };
                    caixa = Some((novo_preco_caixa, custo_caixa, (fator > 1.0).then_some(fator)));
                }

                sqlx::query(
                    r#"UPDATE "Product" SET
                         "stock" = "stock" + ?, "costPrice" = ?, "price" = ?,
                         "boxPrice" = CASE WHEN ? THEN ? ELSE "boxPrice" END,
                         "boxCostPrice" = CASE WHEN ? THEN ? ELSE "boxCostPrice" END,
                         "boxQuantity" = COALESCE(?, "boxQuantity"),
                         "supplierId" = COALESCE(?, "supplierId"), "supplier" = COALESCE(?, "supplier"),
                         "ncm" = COALESCE(?, "ncm"), "cfop" = COALESCE(?, "cfop"), "ean" = COALESCE(?, "ean"),
                         "updatedAt" = ?
                       WHERE "id" = ?"#,
                )
                .bind(qtd)
                .bind(custo)
                .bind(preco)
                .bind(caixa.is_some())
                .bind(caixa.map(|c| c.0))
                .bind(caixa.is_some())
                .bind(caixa.map(|c| c.1))
                // a afinidade INTEGER da coluna guarda 12.0 como 12 e mantém fração
                .bind(caixa.and_then(|c| c.2))
                .bind(fornecedor_id)
                .bind(fornecedor_nome)
                .bind(texto_ou_nulo(&x.ncm))
                .bind(texto_ou_nulo(&x.cfop))
                .bind(texto_ou_nulo(&x.ean))
                .bind(agora)
                .bind(product_id)
                .execute(&mut **tx)
                .await?;

                if custo_antigo != Some(custo) || (preco_antigo - preco).abs() > 0.001 {
                    gravar_historico(
                        &mut tx, product_id, custo_antigo, custo, preco_antigo, preco, usuario, fator, chave.as_deref(), false,
                    )
                    .await?;
                }
                res.updated += 1;
            }
            ("NEW", _, Some(category_id)) => {
                let codigo = proximo_codigo_categoria(&mut tx, category_id).await?;
                let margem = item.target_margin.filter(|m| *m != 0.0).unwrap_or(50.0);
                let preco = match item.new_sale_price {
                    Some(p) if p != 0.0 && p.is_finite() => p,
                    _ => preco_pela_margem(custo, margem),
                };
                let tem_caixa = fator > 1.0 || item.update_box_price;
                let unidades = if fator > 1.0 { fator } else { 24.0 };
                let custo_caixa = if custo_nota > 0.0 { custo_nota } else { round2(custo * unidades) };
                let preco_caixa = item.new_box_price.filter(|p| *p != 0.0).unwrap_or_else(|| round2(preco * unidades));
                let id = ids::cuid();
                sqlx::query(
                    r#"INSERT INTO "Product" ("id", "name", "code", "ean", "supplier", "supplierId", "ncm", "cfop", "unit",
                         "price", "costPrice", "targetMargin", "stock", "categoryId", "hasBoxPrice", "boxQuantity",
                         "boxCostPrice", "boxPrice", "createdAt", "updatedAt")
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                )
                .bind(&id)
                .bind(&x.name)
                .bind(&codigo)
                .bind(texto_ou_nulo(&x.ean))
                .bind(fornecedor_nome)
                .bind(fornecedor_id)
                .bind(texto_ou_nulo(&x.ncm))
                .bind(texto_ou_nulo(&x.cfop))
                .bind(texto_ou_nulo(&x.unit).unwrap_or("un"))
                .bind(preco)
                .bind(custo)
                .bind(margem)
                .bind(qtd)
                .bind(category_id)
                .bind(tem_caixa)
                .bind(tem_caixa.then_some(unidades))
                .bind(tem_caixa.then_some(custo_caixa))
                .bind(tem_caixa.then_some(preco_caixa))
                .bind(agora)
                .bind(agora)
                .execute(&mut **tx)
                .await?;
                gravar_historico(&mut tx, &id, None, custo, preco, preco, usuario, fator, chave.as_deref(), true).await?;
                res.created += 1;
            }
            _ => {}
        }
    }

    // A nota foi conferida e entrou no estoque: NotaRecebida finalizada (criada se ainda não existia)
    if let Some(chave) = &chave {
        let valor = imp.valor_total.unwrap_or(0.0);
        let r = sqlx::query(
            r#"UPDATE "NotaRecebida" SET "status" = 'finalizada',
                 "valorTotal" = CASE WHEN ?1 > 0 THEN ?1 ELSE "valorTotal" END,
                 "emitente" = COALESCE(?2, "emitente"), "numero" = COALESCE(?3, "numero"),
                 "serie" = COALESCE(?4, "serie"), "dataEmissao" = COALESCE(?5, "dataEmissao")
               WHERE "chave" = ?6 OR REPLACE("chave", ' ', '') = ?6"#,
        )
        .bind(valor)
        .bind(texto_ou_nulo(&imp.vendor_name))
        .bind(&imp.numero)
        .bind(&imp.serie)
        .bind(texto_ou_nulo(&imp.data_emissao))
        .bind(chave)
        .execute(&mut **tx)
        .await?;
        if r.rows_affected() == 0 {
            sqlx::query(
                r#"INSERT INTO "NotaRecebida" ("id", "chave", "emitente", "cnpjEmitente", "numero", "serie", "dataEmissao",
                                               "valorTotal", "status", "xmlContent", "createdAt")
                   VALUES (?1, ?1, ?2, NULL, ?3, ?4, ?5, ?6, 'finalizada', NULL, ?7)"#,
            )
            .bind(chave)
            .bind(texto_ou_nulo(&imp.vendor_name).unwrap_or("Fornecedor"))
            .bind(imp.numero.clone().unwrap_or_default())
            .bind(imp.serie.clone().unwrap_or_default())
            .bind(
                texto_ou_nulo(&imp.data_emissao)
                    .map(str::to_string)
                    .unwrap_or_else(|| agora.0.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            )
            .bind(valor)
            .bind(agora)
            .execute(&mut **tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(res)
}

// ============================================================
// Produtos para as telas fiscais (conferência da nota e emissor de NFC-e)
// ============================================================

/// Produto com os campos que a conferência e o emissor precisam (o catálogo de venda não traz custo,
/// margem nem dados fiscais).
#[derive(Debug, Clone, Default, PartialEq, Serialize, sqlx::FromRow)]
pub struct ProdutoFiscal {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
    pub ean: Option<String>,
    pub brand: Option<String>,
    pub price: f64,
    #[sqlx(rename = "costPrice")]
    pub cost_price: Option<f64>,
    #[sqlx(rename = "targetMargin")]
    pub target_margin: Option<f64>,
    #[sqlx(rename = "hasBoxPrice")]
    pub has_box_price: bool,
    #[sqlx(rename = "boxQuantity")]
    pub box_quantity: Option<f64>,
    #[sqlx(rename = "boxPrice")]
    pub box_price: Option<f64>,
    pub stock: f64,
    pub ncm: Option<String>,
    pub cfop: Option<String>,
    pub cest: Option<String>,
    pub unit: String,
}

/// Produtos ativos em ordem alfabética.
pub async fn produtos_fiscais(db: &Db) -> DbResult<Vec<ProdutoFiscal>> {
    Ok(sqlx::query_as(
        r#"SELECT "id", "name", "code", "ean", "brand", CAST("price" AS REAL) AS "price", CAST("costPrice" AS REAL) AS "costPrice",
                  CAST("targetMargin" AS REAL) AS "targetMargin", "hasBoxPrice", CAST("boxQuantity" AS REAL) AS "boxQuantity",
                  CAST("boxPrice" AS REAL) AS "boxPrice", CAST("stock" AS REAL) AS "stock", "ncm", "cfop", "cest", "unit"
           FROM "Product" WHERE "isActive" = 1 ORDER BY "name" COLLATE NOCASE"#,
    )
    .fetch_all(db.pool())
    .await?)
}

/// Margem de lucro padrão do sistema (`SystemSettings.defaultProfitMargin`), 50% se não houver.
pub async fn margem_padrao(db: &Db) -> DbResult<f64> {
    let m: Result<Option<f64>, sqlx::Error> =
        sqlx::query_scalar(r#"SELECT CAST("defaultProfitMargin" AS REAL) FROM "SystemSettings" LIMIT 1"#).fetch_optional(db.pool()).await;
    Ok(m.ok().flatten().filter(|m| *m > 0.0).unwrap_or(50.0))
}

/// Grava o NCM no cadastro do produto (cadastro feito na hora da emissão da NFC-e).
pub async fn salvar_ncm(db: &Db, product_id: &str, ncm: &str) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    let n = sqlx::query(r#"UPDATE "Product" SET "ncm" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(ncm)
        .bind(DbTime::now())
        .bind(product_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    if n == 0 {
        return Err(DbError::not_found("Produto não encontrado."));
    }
    Ok(())
}

// ============================================================
// Posição de estoque (painel do contador)
// ============================================================

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EstoqueCategoria {
    pub nome: String,
    pub itens: u32,
    /// custo × estoque
    pub custo: f64,
    /// preço × estoque
    pub venda: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PosicaoEstoque {
    pub custo_total: f64,
    pub venda_total: f64,
    /// Na ordem em que a categoria aparece nos produtos.
    pub categorias: Vec<EstoqueCategoria>,
}

/// Valor do estoque dos produtos ativos (estoque negativo conta como zero), por categoria.
pub async fn posicao_estoque(db: &Db) -> DbResult<PosicaoEstoque> {
    let linhas: Vec<(f64, f64, f64, Option<String>)> = sqlx::query_as(
        r#"SELECT CAST(p."stock" AS REAL), CAST(COALESCE(p."costPrice", 0) AS REAL), CAST(p."price" AS REAL), c."name"
           FROM "Product" p LEFT JOIN "Category" c ON c."id" = p."categoryId"
           WHERE p."isActive" = 1 ORDER BY p.rowid"#,
    )
    .fetch_all(db.pool())
    .await?;
    let mut pos = PosicaoEstoque::default();
    for (estoque, custo_un, preco, categoria) in linhas {
        let estoque = estoque.max(0.0);
        let (custo, venda) = (custo_un * estoque, preco * estoque);
        pos.custo_total += custo;
        pos.venda_total += venda;
        let nome = categoria.unwrap_or_else(|| "Geral".into());
        let idx = match pos.categorias.iter().position(|c| c.nome == nome) {
            Some(i) => i,
            None => {
                pos.categorias.push(EstoqueCategoria { nome, ..Default::default() });
                pos.categorias.len() - 1
            }
        };
        let c = &mut pos.categorias[idx];
        c.itens += 1;
        c.custo += custo;
        c.venda += venda;
    }
    Ok(pos)
}

// ============================================================
// Ajustes de impressão do DANFE da NFC-e
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AjustesImpressao {
    pub paper_width: f64,
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
    pub margin_right: f64,
    pub font_scale: f64,
    pub qr_size: f64,
}

impl Default for AjustesImpressao {
    fn default() -> Self {
        Self {
            paper_width: 80.0,
            margin_top: 2.0,
            margin_bottom: 12.0,
            margin_left: 1.0,
            margin_right: 1.0,
            font_scale: 100.0,
            qr_size: 100.0,
        }
    }
}

/// `PrinterSettings` (só leitura). Zero, vazio ou tabela inexistente usam os padrões, como no Node.
pub async fn ler_ajustes_impressao_danfe(db: &Db) -> DbResult<AjustesImpressao> {
    type Linha = (Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>);
    let linha: Result<Option<Linha>, sqlx::Error> = sqlx::query_as(
        r#"SELECT CAST("paperWidth" AS REAL), CAST("marginTop" AS REAL), CAST("marginBottom" AS REAL),
                  CAST("marginLeft" AS REAL), CAST("marginRight" AS REAL), CAST("fontScale" AS REAL), CAST("qrSize" AS REAL)
           FROM "PrinterSettings" WHERE "id" = 'default' LIMIT 1"#,
    )
    .fetch_optional(db.pool())
    .await;
    let p = AjustesImpressao::default();
    let Ok(Some(l)) = linha else {
        return Ok(p);
    };
    let ou = |v: Option<f64>, d: f64| v.filter(|x| *x != 0.0 && x.is_finite()).unwrap_or(d);
    Ok(AjustesImpressao {
        paper_width: ou(l.0, p.paper_width),
        margin_top: ou(l.1, p.margin_top),
        margin_bottom: ou(l.2, p.margin_bottom),
        margin_left: ou(l.3, p.margin_left),
        margin_right: ou(l.4, p.margin_right),
        font_scale: ou(l.5, p.font_scale),
        qr_size: ou(l.6, p.qr_size),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notas;

    const CHAVE: &str = "15260936275163000124550020000000011794431341";

    async fn base(db: &Db) {
        for (id, nome, ordem, inicio) in
            [("beb", "Bebidas", 1, None::<i64>), ("coz", "Cozinha", 2, Some(0)), ("out", "Outros", 3, Some(0))]
        {
            sqlx::query(r#"INSERT INTO "Category" ("id", "name", "sortOrder", "codeStart") VALUES (?, ?, ?, ?)"#)
                .bind(id)
                .bind(nome)
                .bind(ordem)
                .bind(inicio)
                .execute(db.pool())
                .await
                .unwrap();
        }
        sqlx::query(
            r#"INSERT INTO "Product" ("id", "name", "code", "price", "costPrice", "targetMargin", "hasBoxPrice", "boxQuantity",
                                      "boxPrice", "categoryId", "stock", "createdAt", "updatedAt")
               VALUES ('lata', 'Cerveja Lata', '5001', 6.0, 3.0, 40.0, 1, 12, 60.0, 'beb', 10, 0, 0)"#,
        )
        .execute(db.pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn codigo_sequencial() {
        let db = Db::open_in_memory().await.unwrap();
        base(&db).await;
        let mut c = db.pool().acquire().await.unwrap();
        assert_eq!(proximo_codigo_categoria(&mut c, "beb").await.unwrap(), "5002"); // 5001 já usado
        assert_eq!(proximo_codigo_categoria(&mut c, "coz").await.unwrap(), "1001");
        assert_eq!(proximo_codigo_categoria(&mut c, "out").await.unwrap(), "3001");
        assert_eq!(proximo_codigo_categoria(&mut c, "nada").await.unwrap(), "1001");
    }

    #[tokio::test]
    async fn fornecedor_pelo_documento() {
        let db = Db::open_in_memory().await.unwrap();
        let f = FornecedorNfe {
            documento: "36.275.163/0001-24".into(),
            nome: "Distribuidora".into(),
            fantasia: Some("Distri".into()),
            cidade: Some("Belém".into()),
            ..Default::default()
        };
        let id = upsert_fornecedor_nfe(&db, &f).await.unwrap().unwrap();
        let id2 = upsert_fornecedor_nfe(&db, &FornecedorNfe { nome: "Distribuidora LTDA".into(), ..f.clone() }).await.unwrap();
        assert_eq!(Some(id.clone()), id2);
        // o bip só traz o nome: o resto do cadastro fica
        upsert_fornecedor_nfe(&db, &FornecedorNfe { documento: "36275163000124".into(), nome: "D".into(), ..Default::default() })
            .await
            .unwrap();
        let (nome, fantasia, cidade): (String, Option<String>, Option<String>) =
            sqlx::query_as(r#"SELECT "name", "tradeName", "city" FROM "Supplier" WHERE "id" = ?"#)
                .bind(&id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!((nome.as_str(), fantasia.as_deref(), cidade.as_deref()), ("D", Some("Distri"), Some("Belém")));
        assert_eq!(upsert_fornecedor_nfe(&db, &FornecedorNfe::default()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn entrada_da_nota() {
        let db = Db::open_in_memory().await.unwrap();
        base(&db).await;
        notas::gravar_recebida(&db, &notas::RegistroRecebida { chave: CHAVE.into(), ..Default::default() }, None)
            .await
            .unwrap();
        let corpo = serde_json::json!({
            "items": [
                { "xmlItem": { "name": "CERVEJA LATA CX12", "quantity": "2", "unitCost": 48.0, "ncm": "22030000" },
                  "action": "LINK", "productId": "lata", "conversionFactor": 12, "updateBoxPrice": true },
                { "xmlItem": { "name": "AMENDOIM", "quantity": 5, "unitCost": 2.5, "unit": "pc" },
                  "action": "NEW", "categoryId": "coz", "targetMargin": 60 },
                { "xmlItem": { "name": "IGNORADO" }, "action": "SKIP" }
            ],
            "chaveAcesso": CHAVE, "vendorName": "Distribuidora", "valorTotal": "108.50", "numero": 1, "serie": "2"
        });
        let imp: ImportacaoXml = serde_json::from_value(corpo).unwrap();
        let r = aplicar_importacao_xml(&db, &imp, Some("Ana")).await.unwrap();
        assert_eq!(r, ResultadoImportacao { updated: 1, created: 1 });

        // 2 caixas × 12 = 24 latas a R$ 4,00; preço pela margem de 40% = 6,67; caixa proporcional 60 × 6,67/6
        let (estoque, custo, preco, preco_cx, custo_cx, qtd_cx, ncm, fornecedor): (i64, f64, f64, f64, f64, i64, String, String) =
            sqlx::query_as(
                r#"SELECT "stock", "costPrice", "price", "boxPrice", "boxCostPrice", "boxQuantity", "ncm", "supplier"
                   FROM "Product" WHERE "id" = 'lata'"#,
            )
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!((estoque, custo, preco), (34, 4.0, 6.67));
        assert_eq!((preco_cx, custo_cx, qtd_cx), (66.7, 48.0, 12));
        assert_eq!((ncm.as_str(), fornecedor.as_str()), ("22030000", "Distribuidora"));

        let (codigo, preco_novo, estoque_novo, unidade): (String, f64, i64, String) =
            sqlx::query_as(r#"SELECT "code", "price", "stock", "unit" FROM "Product" WHERE "name" = 'AMENDOIM'"#)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!((codigo.as_str(), preco_novo, estoque_novo, unidade.as_str()), ("1001", 6.25, 5, "pc"));

        let hist: Vec<(String, String, Option<String>, Option<f64>)> = sqlx::query_as(
            r#"SELECT "changedBy", "reason", "nfeChave", "costDiff" FROM "PriceHistory" ORDER BY "reason" DESC, "costDiff""#,
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(hist.len(), 2);
        assert!(hist.iter().all(|h| h.0 == "Ana" && h.2.as_deref() == Some(CHAVE)));
        assert!(hist.iter().any(|h| h.1 == "IMPORT_XML (Fator x12)" && h.3 == Some(1.0)));

        let nota = notas::buscar_recebida(&db, CHAVE).await.unwrap().unwrap();
        assert_eq!((nota.status.as_str(), nota.valor_total, nota.numero.as_deref()), ("finalizada", Some(108.5), Some("1")));

        // produto inexistente: nada é gravado
        let ruim: ImportacaoXml = serde_json::from_value(serde_json::json!({
            "items": [ { "xmlItem": { "name": "X", "quantity": 1 }, "action": "NEW", "categoryId": "beb" },
                       { "xmlItem": { "name": "Y", "quantity": 1 }, "action": "LINK", "productId": "nao-existe" } ]
        }))
        .unwrap();
        assert!(matches!(aplicar_importacao_xml(&db, &ruim, None).await, Err(DbError::NotFound(_))));
        let (n,): (i64,) = sqlx::query_as(r#"SELECT COUNT(*) FROM "Product""#).fetch_one(db.pool()).await.unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test]
    async fn produtos_margem_e_ncm() {
        let db = Db::open_in_memory().await.unwrap();
        base(&db).await;
        let p = produtos_fiscais(&db).await.unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].name.as_str(), p[0].cost_price, p[0].target_margin, p[0].box_quantity, p[0].stock), ("Cerveja Lata", Some(3.0), Some(40.0), Some(12.0), 10.0));
        assert_eq!(margem_padrao(&db).await.unwrap(), 50.0);
        salvar_ncm(&db, "lata", "22030000").await.unwrap();
        assert_eq!(produtos_fiscais(&db).await.unwrap()[0].ncm.as_deref(), Some("22030000"));
        assert!(matches!(salvar_ncm(&db, "nada", "1").await, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn estoque_e_impressao() {
        let db = Db::open_in_memory().await.unwrap();
        base(&db).await;
        sqlx::query(
            r#"INSERT INTO "Product" ("id", "name", "price", "costPrice", "categoryId", "stock", "createdAt", "updatedAt")
               VALUES ('neg', 'Negativo', 5, 2, 'coz', -3, 0, 0), ('frac', 'Fração', 10, 4, 'beb', 1.5, 0, 0)"#,
        )
        .execute(db.pool())
        .await
        .unwrap();
        let p = posicao_estoque(&db).await.unwrap();
        assert_eq!((p.custo_total, p.venda_total), (30.0 + 6.0, 60.0 + 15.0));
        assert_eq!(p.categorias.iter().map(|c| (c.nome.as_str(), c.itens)).collect::<Vec<_>>(), [("Bebidas", 2), ("Cozinha", 1)]);

        assert_eq!(ler_ajustes_impressao_danfe(&db).await.unwrap(), AjustesImpressao::default());
        sqlx::query(r#"CREATE TABLE "PrinterSettings" ("id" TEXT PRIMARY KEY, "paperWidth" REAL, "marginTop" REAL, "marginBottom" REAL,
                         "marginLeft" REAL, "marginRight" REAL, "fontScale" REAL, "qrSize" REAL)"#)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(r#"INSERT INTO "PrinterSettings" VALUES ('default', 58, 0, 5, 1, 1, 90, '120')"#)
            .execute(db.pool())
            .await
            .unwrap();
        let a = ler_ajustes_impressao_danfe(&db).await.unwrap();
        assert_eq!((a.paper_width, a.margin_top, a.margin_bottom, a.font_scale, a.qr_size), (58.0, 2.0, 5.0, 90.0, 120.0));
    }
}
