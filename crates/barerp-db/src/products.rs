//! Cadastro de produtos e categorias (tela Cardápio & Estoque). Tradução de `server/src/routes/products.ts`
//! (rotas de produto, categoria, códigos internos, estoque e histórico de preços), com estas diferenças de propósito:
//! - excluir produto que já foi vendido só o **desativa** (o Node apagava os itens das comandas antigas,
//!   inclusive pagas, e corrompia relatórios, caixa e fiscal); produto nunca vendido é apagado;
//! - excluir categoria com produto já vendido é recusado (mova os produtos antes); os nunca vendidos são apagados;
//! - produto que é insumo da ficha técnica de outro não é apagado (o Node dava erro 500);
//! - o ajuste rápido de estoque existe (a rota `PUT /products/:id/stock` que a tela chamava não existia no Node);
//! - o relatório de estoque traz o resumo e o nome das categorias (a tela do Node lia campos que a rota não mandava);
//! - tudo numa transação.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use barerp_core::ids;
use barerp_core::pricing::round2;

use crate::events::Event;
use crate::{importacao, Db, DbError, DbResult, DbTime};

// ============================================================
// Leitura
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ProductRow {
    pub id: String,
    pub code: Option<String>,
    pub ean: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub brand: Option<String>,
    /// Nome avulso do fornecedor (texto livre)
    pub supplier: Option<String>,
    #[sqlx(rename = "supplierId")]
    pub supplier_id: Option<String>,
    /// Fantasia ou razão do fornecedor cadastrado
    #[sqlx(rename = "supplierName")]
    pub supplier_name: Option<String>,
    #[sqlx(rename = "categoryId")]
    pub category_id: String,
    #[sqlx(rename = "categoryName")]
    pub category_name: Option<String>,
    #[sqlx(rename = "kdsStation")]
    pub kds_station: String,
    pub price: f64,
    #[sqlx(rename = "costPrice")]
    pub cost_price: Option<f64>,
    #[sqlx(rename = "targetMargin")]
    pub target_margin: Option<f64>,
    pub stock: i64,
    #[sqlx(rename = "minStock")]
    pub min_stock: i64,
    #[sqlx(rename = "trackStock")]
    pub track_stock: bool,
    pub unit: String,
    #[sqlx(rename = "hasBoxPrice")]
    pub has_box_price: bool,
    #[sqlx(rename = "boxQuantity")]
    pub box_quantity: Option<i64>,
    #[sqlx(rename = "boxPrice")]
    pub box_price: Option<f64>,
    #[sqlx(rename = "boxCostPrice")]
    pub box_cost_price: Option<f64>,
    #[sqlx(rename = "boxEan")]
    pub box_ean: Option<String>,
    pub ncm: Option<String>,
    pub cfop: Option<String>,
    pub cest: Option<String>,
    #[sqlx(rename = "isActive")]
    pub is_active: bool,
    #[sqlx(rename = "showInMenu")]
    pub show_in_menu: bool,
    #[sqlx(rename = "imageHash")]
    pub image_hash: Option<String>,
    /// Insumos da ficha técnica (0 = produto simples)
    #[sqlx(rename = "componentCount")]
    pub component_count: i64,
}

impl ProductRow {
    /// Estoque no mínimo ou abaixo (alerta de reposição).
    pub fn is_low_stock(&self) -> bool {
        self.stock <= self.min_stock
    }

    /// Margem sobre a venda, arredondada (só com custo informado).
    pub fn margin_percent(&self) -> Option<i64> {
        match self.cost_price {
            Some(c) if self.price > 0.0 => Some((((self.price - c) / self.price) * 100.0).round() as i64),
            _ => None,
        }
    }

    /// Fornecedor para mostrar: o cadastrado ou o nome avulso.
    pub fn supplier_label(&self) -> Option<&str> {
        self.supplier_name.as_deref().or(self.supplier.as_deref()).filter(|s| !s.is_empty())
    }
}

const PRODUCT_SELECT: &str = r#"SELECT p."id", p."code", p."ean", p."name", p."description", p."brand", p."supplier",
       p."supplierId", COALESCE(NULLIF(s."tradeName", ''), s."name") AS "supplierName",
       p."categoryId", c."name" AS "categoryName", p."kdsStation", p."price", p."costPrice", p."targetMargin",
       CAST(p."stock" AS INTEGER) AS "stock", CAST(p."minStock" AS INTEGER) AS "minStock", p."trackStock", p."unit",
       p."hasBoxPrice", p."boxQuantity", p."boxPrice", p."boxCostPrice", p."boxEan", p."ncm", p."cfop", p."cest",
       p."isActive", p."showInMenu", i."hash" AS "imageHash",
       (SELECT COUNT(*) FROM "ProductComponent" pc WHERE pc."parentProductId" = p."id") AS "componentCount"
FROM "Product" p
LEFT JOIN "Category" c ON c."id" = p."categoryId"
LEFT JOIN "Supplier" s ON s."id" = p."supplierId"
LEFT JOIN "ProductImage" i ON i."productId" = p."id""#;

/// Produtos ativos, na ordem do Node (categoria, nome).
pub async fn list(db: &Db) -> DbResult<Vec<ProductRow>> {
    Ok(sqlx::query_as(&format!(r#"{PRODUCT_SELECT} WHERE p."isActive" = 1 ORDER BY p."categoryId", p."name""#))
        .fetch_all(db.pool())
        .await?)
}

async fn row(conn: &mut SqliteConnection, id: &str) -> DbResult<ProductRow> {
    sqlx::query_as(&format!(r#"{PRODUCT_SELECT} WHERE p."id" = ?"#))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DbError::not_found("Produto não encontrado"))
}

/// Insumo da ficha técnica.
#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ComponentLine {
    #[sqlx(rename = "componentId")]
    pub component_id: String,
    pub name: String,
    pub unit: String,
    pub quantity: f64,
    #[sqlx(rename = "costPrice")]
    pub cost_price: Option<f64>,
    pub price: f64,
    pub stock: i64,
}

impl ComponentLine {
    /// Custo do insumo usado na receita: custo, ou a venda quando não tem custo (como a tela do Node).
    pub fn unit_cost(&self) -> f64 {
        self.cost_price.filter(|c| *c > 0.0).unwrap_or(self.price)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductDetail {
    pub product: ProductRow,
    pub components: Vec<ComponentLine>,
    /// Grupos de adicionais do produto, na ordem de exibição (inclui inativos, para não perder o vínculo)
    pub modifier_group_ids: Vec<String>,
}

pub async fn detail(db: &Db, id: &str) -> DbResult<ProductDetail> {
    let mut conn = db.pool().acquire().await?;
    let product = row(&mut conn, id).await?;
    let components = sqlx::query_as(
        r#"SELECT pc."componentId", p."name", p."unit", pc."quantity", p."costPrice", p."price",
                  CAST(p."stock" AS INTEGER) AS "stock"
           FROM "ProductComponent" pc JOIN "Product" p ON p."id" = pc."componentId"
           WHERE pc."parentProductId" = ? ORDER BY pc."createdAt", p."name""#,
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let modifier_group_ids = sqlx::query_scalar(
        r#"SELECT "groupId" FROM "ProductModifierGroup" WHERE "productId" = ? ORDER BY "sortOrder", "groupId""#,
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ProductDetail { product, components, modifier_group_ids })
}

/// Quantos produtos ativos repetem um código interno (soma de todos os repetidos, como o aviso da tela).
pub fn duplicate_code_count(products: &[ProductRow]) -> usize {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for p in products {
        if let Some(c) = p.code.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            *count.entry(c).or_default() += 1;
        }
    }
    count.values().filter(|n| **n > 1).sum()
}

// ============================================================
// Gravação de produto
// ============================================================

/// Formulário do produto (o mesmo para criar e alterar).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductInput {
    pub name: String,
    pub code: Option<String>,
    pub ean: Option<String>,
    pub brand: Option<String>,
    /// Nome avulso; vazio com `supplier_id` usa o nome do cadastrado
    pub supplier: Option<String>,
    pub supplier_id: Option<String>,
    pub description: Option<String>,
    pub show_in_menu: bool,
    pub price: f64,
    pub cost_price: Option<f64>,
    pub target_margin: Option<f64>,
    pub category_id: String,
    /// BAR, KITCHEN ou NONE
    pub kds_station: String,
    pub stock: f64,
    pub min_stock: f64,
    pub ncm: Option<String>,
    pub cfop: Option<String>,
    pub cest: Option<String>,
    pub unit: String,
    pub has_box_price: bool,
    pub box_quantity: Option<i64>,
    pub box_price: Option<f64>,
    pub box_ean: Option<String>,
    /// Ficha técnica: (produto insumo, quantidade). Repetidos são somados; quantidade ≤ 0 é ignorada.
    pub components: Vec<(String, f64)>,
}

fn text(v: &Option<String>) -> Option<String> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn merged_components(list: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut order: Vec<String> = Vec::new();
    let mut total: HashMap<String, f64> = HashMap::new();
    for (id, qty) in list {
        if id.is_empty() || !qty.is_finite() || *qty <= 0.0 {
            continue;
        }
        if !total.contains_key(id) {
            order.push(id.clone());
        }
        *total.entry(id.clone()).or_default() += qty;
    }
    order.into_iter().map(|id| {
        let q = (total[&id] * 10_000.0).round() / 10_000.0;
        (id, q)
    }).collect()
}

/// Liga o nome avulso ao fornecedor cadastrado (como a tela do Node fazia antes de salvar).
async fn resolve_supplier(conn: &mut SqliteConnection, input: &ProductInput) -> DbResult<(Option<String>, Option<String>)> {
    let name = text(&input.supplier);
    let id = text(&input.supplier_id);
    match (&name, &id) {
        (None, Some(id)) => {
            let found: Option<String> = sqlx::query_scalar(
                r#"SELECT COALESCE(NULLIF("tradeName", ''), "name") FROM "Supplier" WHERE "id" = ?"#,
            )
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
            Ok((found, Some(id.clone())))
        }
        (Some(n), None) => {
            let found: Option<String> = sqlx::query_scalar(
                r#"SELECT "id" FROM "Supplier" WHERE lower("name") = lower(?) OR lower("tradeName") = lower(?) LIMIT 1"#,
            )
            .bind(n)
            .bind(n)
            .fetch_optional(&mut *conn)
            .await?;
            Ok((name.clone(), found))
        }
        _ => Ok((name, id)),
    }
}

fn validate(input: &ProductInput) -> DbResult<()> {
    if input.name.trim().is_empty() || input.category_id.trim().is_empty() || !input.price.is_finite() || input.price < 0.0 {
        return Err(DbError::rule("Preencha os campos obrigatórios (Nome, Preço e Categoria)"));
    }
    Ok(())
}

async fn write_components(conn: &mut SqliteConnection, product_id: &str, input: &ProductInput) -> DbResult<()> {
    sqlx::query(r#"DELETE FROM "ProductComponent" WHERE "parentProductId" = ?"#).bind(product_id).execute(&mut *conn).await?;
    let now = DbTime::now();
    for (component_id, quantity) in merged_components(&input.components) {
        if component_id == product_id {
            return Err(DbError::rule("O produto não pode ser insumo da própria ficha técnica."));
        }
        sqlx::query(
            r#"INSERT INTO "ProductComponent" ("id", "parentProductId", "componentId", "quantity", "createdAt", "updatedAt")
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(ids::cuid())
        .bind(product_id)
        .bind(&component_id)
        .bind(quantity)
        .bind(now)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn record_price_history(
    conn: &mut SqliteConnection,
    product_id: &str,
    old_cost: Option<f64>,
    new_cost: Option<f64>,
    old_price: f64,
    new_price: f64,
    user: Option<&str>,
    reason: &str,
) -> DbResult<()> {
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let cost_diff = match (new_cost, old_cost) {
        (Some(n), Some(o)) => Some(r2(n - o)),
        (Some(n), None) => Some(n),
        _ => None,
    };
    let cost_percent = match (old_cost, new_cost) {
        (Some(o), Some(n)) if o > 0.0 => Some(r2((n - o) / o * 100.0)),
        _ => None,
    };
    let price_diff = r2(new_price - old_price);
    let price_percent = if old_price > 0.0 { r2((new_price - old_price) / old_price * 100.0) } else { 0.0 };
    sqlx::query(
        r#"INSERT INTO "PriceHistory" ("id", "productId", "oldCostPrice", "newCostPrice", "oldPrice", "newPrice", "costDiff",
                                       "costPercent", "priceDiff", "pricePercent", "changedBy", "reason", "createdAt")
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
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
    .bind(user.filter(|u| !u.trim().is_empty()).unwrap_or("Operador"))
    .bind(reason)
    .bind(DbTime::now())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn catalog_event() -> Vec<Event> {
    vec![Event::CatalogUpdated { scope: "products" }]
}

/// Cadastra o produto. Código vazio ou já usado recebe o próximo da faixa da categoria. Devolve o id.
pub async fn create(db: &Db, input: ProductInput, user: Option<&str>) -> DbResult<(String, Vec<Event>)> {
    validate(&input)?;
    let mut tx = db.begin_write().await?;
    let mut code = text(&input.code).map(|c| c.to_uppercase());
    if let Some(c) = &code {
        let used: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Product" WHERE "code" = ?"#)
            .bind(c)
            .fetch_optional(&mut **tx)
            .await?;
        if used.is_some() {
            code = None;
        }
    }
    let code = match code {
        Some(c) => c,
        None => importacao::proximo_codigo_categoria(&mut tx, &input.category_id).await?,
    };
    let (supplier, supplier_id) = resolve_supplier(&mut tx, &input).await?;
    let id = ids::cuid();
    let now = DbTime::now();
    let cost = input.cost_price.filter(|c| c.is_finite());
    sqlx::query(
        r#"INSERT INTO "Product" ("id", "name", "code", "ean", "supplier", "supplierId", "brand", "ncm", "cfop", "cest", "unit",
               "description", "price", "costPrice", "targetMargin", "hasBoxPrice", "boxQuantity", "boxPrice", "boxEan",
               "categoryId", "kdsStation", "stock", "minStock", "showInMenu", "createdAt", "updatedAt")
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(input.name.trim())
    .bind(&code)
    .bind(text(&input.ean))
    .bind(supplier)
    .bind(supplier_id)
    .bind(text(&input.brand))
    .bind(text(&input.ncm))
    .bind(text(&input.cfop))
    .bind(text(&input.cest))
    .bind(Some(input.unit.trim()).filter(|u| !u.is_empty()).unwrap_or("un"))
    .bind(text(&input.description))
    .bind(input.price)
    .bind(cost)
    .bind(input.target_margin.filter(|m| m.is_finite()))
    .bind(input.has_box_price)
    .bind(if input.has_box_price { input.box_quantity.filter(|q| *q > 0).or(Some(24)) } else { Some(24) })
    .bind(if input.has_box_price { input.box_price } else { None })
    .bind(if input.has_box_price { text(&input.box_ean) } else { None })
    .bind(&input.category_id)
    .bind(Some(input.kds_station.as_str()).filter(|k| !k.is_empty()).unwrap_or("BAR"))
    .bind(input.stock.round() as i64)
    .bind(input.min_stock.round() as i64)
    .bind(input.show_in_menu)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    write_components(&mut tx, &id, &input).await?;
    record_price_history(&mut tx, &id, None, cost, input.price, input.price, user, "CADASTRO_INICIAL").await?;
    tx.commit().await?;
    Ok((id, catalog_event()))
}

/// Altera o produto. Grava no histórico quando venda ou custo mudam.
pub async fn update(db: &Db, id: &str, input: ProductInput, user: Option<&str>) -> DbResult<Vec<Event>> {
    validate(&input)?;
    let mut tx = db.begin_write().await?;
    let old: Option<(f64, Option<f64>)> = sqlx::query_as(r#"SELECT "price", "costPrice" FROM "Product" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    let (old_price, old_cost) = old.ok_or_else(|| DbError::not_found("Produto não encontrado"))?;
    let code = text(&input.code).map(|c| c.to_uppercase());
    if let Some(c) = &code {
        let other: Option<String> = sqlx::query_scalar(r#"SELECT "name" FROM "Product" WHERE "code" = ? AND "id" <> ?"#)
            .bind(c)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
        if let Some(name) = other {
            return Err(DbError::rule(format!(r#"O código #{c} já está em uso pelo produto "{name}"."#)));
        }
    }
    let (supplier, supplier_id) = resolve_supplier(&mut tx, &input).await?;
    let cost = input.cost_price.filter(|c| c.is_finite());
    sqlx::query(
        r#"UPDATE "Product" SET "name" = ?, "code" = ?, "ean" = ?, "supplier" = ?, "supplierId" = ?, "brand" = ?, "ncm" = ?,
               "cfop" = ?, "cest" = ?, "unit" = ?, "description" = ?, "price" = ?, "costPrice" = ?, "targetMargin" = ?,
               "hasBoxPrice" = ?, "boxQuantity" = ?, "boxPrice" = ?, "boxEan" = ?, "categoryId" = ?, "kdsStation" = ?,
               "stock" = ?, "minStock" = ?, "showInMenu" = ?, "updatedAt" = ?
           WHERE "id" = ?"#,
    )
    .bind(input.name.trim())
    .bind(code)
    .bind(text(&input.ean))
    .bind(supplier)
    .bind(supplier_id)
    .bind(text(&input.brand))
    .bind(text(&input.ncm))
    .bind(text(&input.cfop))
    .bind(text(&input.cest))
    .bind(Some(input.unit.trim()).filter(|u| !u.is_empty()).unwrap_or("un"))
    .bind(text(&input.description))
    .bind(input.price)
    .bind(cost)
    .bind(input.target_margin.filter(|m| m.is_finite()))
    .bind(input.has_box_price)
    .bind(if input.has_box_price { input.box_quantity.filter(|q| *q > 0).or(Some(24)) } else { Some(24) })
    .bind(if input.has_box_price { input.box_price } else { None })
    .bind(if input.has_box_price { text(&input.box_ean) } else { None })
    .bind(&input.category_id)
    .bind(Some(input.kds_station.as_str()).filter(|k| !k.is_empty()).unwrap_or("BAR"))
    .bind(input.stock.round() as i64)
    .bind(input.min_stock.round() as i64)
    .bind(input.show_in_menu)
    .bind(DbTime::now())
    .bind(id)
    .execute(&mut **tx)
    .await?;
    write_components(&mut tx, id, &input).await?;
    let cost_changed = match (old_cost, cost) {
        (Some(a), Some(b)) => (a - b).abs() > 1e-9,
        (None, None) => false,
        _ => true,
    };
    if cost_changed || (old_price - input.price).abs() > 0.001 {
        record_price_history(&mut tx, id, old_cost, cost, old_price, input.price, user, "MANUAL_UPDATE").await?;
    }
    tx.commit().await?;
    Ok(catalog_event())
}

/// Resultado de excluir: apagado ou só desativado (já foi vendido).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Removal {
    Deleted,
    Deactivated,
}

async fn remove_product(conn: &mut SqliteConnection, id: &str, name: &str) -> DbResult<Removal> {
    let open: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM "OrderItem" i JOIN "Order" o ON o."id" = i."orderId" WHERE i."productId" = ? AND o."status" = 'OPEN'"#,
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if open > 0 {
        return Err(DbError::rule(format!(
            r#"Não é possível excluir o produto "{name}" pois ele está em {open} comanda(s) aberta(s) no momento. Feche o atendimento das mesas antes de excluir."#
        )));
    }
    let parents: Vec<String> = sqlx::query_scalar(
        r#"SELECT p."name" FROM "ProductComponent" pc JOIN "Product" p ON p."id" = pc."parentProductId"
           WHERE pc."componentId" = ? AND p."isActive" = 1 ORDER BY p."name""#,
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    if !parents.is_empty() {
        return Err(DbError::rule(format!(
            r#""{name}" é insumo da ficha técnica de {}. Tire da receita antes de excluir."#,
            parents.join(", ")
        )));
    }
    let sold: bool = sqlx::query_scalar(r#"SELECT EXISTS (SELECT 1 FROM "OrderItem" WHERE "productId" = ?)"#)
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    sqlx::query(r#"DELETE FROM "ProductModifierGroup" WHERE "productId" = ?"#).bind(id).execute(&mut *conn).await?;
    sqlx::query(r#"DELETE FROM "ProductImage" WHERE "productId" = ?"#).bind(id).execute(&mut *conn).await?;
    if sold {
        // Mantém o produto para as comandas antigas, relatórios e notas continuarem certos
        sqlx::query(r#"UPDATE "Product" SET "isActive" = 0, "showInMenu" = 0, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(DbTime::now())
            .bind(id)
            .execute(&mut *conn)
            .await?;
        return Ok(Removal::Deactivated);
    }
    sqlx::query(r#"DELETE FROM "ProductComponent" WHERE "parentProductId" = ? OR "componentId" = ?"#)
        .bind(id)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(r#"DELETE FROM "PriceHistory" WHERE "productId" = ?"#).bind(id).execute(&mut *conn).await?;
    sqlx::query(r#"DELETE FROM "Product" WHERE "id" = ?"#).bind(id).execute(&mut *conn).await?;
    Ok(Removal::Deleted)
}

/// Exclui o produto (ou desativa, se já foi vendido). Devolve a mensagem para a tela.
pub async fn delete(db: &Db, id: &str) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let name: String = sqlx::query_scalar(r#"SELECT "name" FROM "Product" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::not_found("Produto não encontrado"))?;
    remove_product(&mut tx, id, &name).await?;
    tx.commit().await?;
    Ok((format!(r#"Produto "{name}" excluído com sucesso!"#), catalog_event()))
}

/// Ativa ou desativa o produto (campo `isActive` do PUT `/products/:id`).
pub async fn set_active(db: &Db, id: &str, active: bool) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let changed = sqlx::query(r#"UPDATE "Product" SET "isActive" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(active)
        .bind(DbTime::now())
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Produto não encontrado"));
    }
    tx.commit().await?;
    Ok(catalog_event())
}

/// Só a descrição (tela "Itens do Cardápio Digital").
pub async fn set_description(db: &Db, id: &str, description: Option<&str>) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let changed = sqlx::query(r#"UPDATE "Product" SET "description" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(description.map(str::trim).filter(|d| !d.is_empty()))
        .bind(DbTime::now())
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Produto não encontrado"));
    }
    tx.commit().await?;
    Ok(catalog_event())
}

/// Ajuste rápido de estoque (−1 / +10 da lista). Devolve o estoque novo.
pub async fn adjust_stock(db: &Db, id: &str, delta: i64) -> DbResult<(i64, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let changed = sqlx::query(r#"UPDATE "Product" SET "stock" = "stock" + ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(delta)
        .bind(DbTime::now())
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Produto não encontrado"));
    }
    let stock: i64 = sqlx::query_scalar(r#"SELECT CAST("stock" AS INTEGER) FROM "Product" WHERE "id" = ?"#)
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    tx.commit().await?;
    Ok((stock, catalog_event()))
}

// ============================================================
// Histórico de preços
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PriceChange {
    pub id: String,
    #[sqlx(rename = "oldCostPrice")]
    pub old_cost_price: Option<f64>,
    #[sqlx(rename = "newCostPrice")]
    pub new_cost_price: Option<f64>,
    #[sqlx(rename = "oldPrice")]
    pub old_price: f64,
    #[sqlx(rename = "newPrice")]
    pub new_price: f64,
    #[sqlx(rename = "costDiff")]
    pub cost_diff: Option<f64>,
    #[sqlx(rename = "priceDiff")]
    pub price_diff: f64,
    #[sqlx(rename = "pricePercent")]
    pub price_percent: f64,
    #[sqlx(rename = "changedBy")]
    pub changed_by: String,
    /// CADASTRO_INICIAL, MANUAL_UPDATE ou IMPORT_XML
    pub reason: String,
    #[sqlx(rename = "nfeChave")]
    pub nfe_chave: Option<String>,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
}

/// Alterações de preço do produto, mais recentes primeiro.
pub async fn price_history(db: &Db, product_id: &str) -> DbResult<Vec<PriceChange>> {
    Ok(sqlx::query_as(
        r#"SELECT "id", "oldCostPrice", "newCostPrice", "oldPrice", "newPrice", "costDiff", "priceDiff", "pricePercent",
                  "changedBy", "reason", "nfeChave", "createdAt"
           FROM "PriceHistory" WHERE "productId" = ? ORDER BY "createdAt" DESC"#,
    )
    .bind(product_id)
    .fetch_all(db.pool())
    .await?)
}

// ============================================================
// Categorias e códigos internos
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRow {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    #[sqlx(rename = "sortOrder")]
    pub sort_order: i64,
    #[sqlx(rename = "codeStart")]
    pub code_start: Option<i64>,
    #[sqlx(rename = "productCount")]
    pub product_count: i64,
}

impl CategoryRow {
    /// Faixa mostrada na tela (`#1001+`).
    pub fn code_start_or_default(&self) -> i64 {
        self.code_start.filter(|c| *c > 0).unwrap_or(1001)
    }
}

pub async fn categories(db: &Db) -> DbResult<Vec<CategoryRow>> {
    Ok(sqlx::query_as(
        r#"SELECT c."id", c."name", c."icon", c."sortOrder", c."codeStart",
                  (SELECT COUNT(*) FROM "Product" p WHERE p."categoryId" = c."id" AND p."isActive" = 1) AS "productCount"
           FROM "Category" c ORDER BY c."sortOrder", c."name""#,
    )
    .fetch_all(db.pool())
    .await?)
}

/// Próxima faixa livre para uma categoria nova (1001, 2001, 3001, ...), como a tela sugere.
pub fn suggest_code_start(categories: &[CategoryRow]) -> i64 {
    let used: HashSet<i64> = categories.iter().filter_map(|c| c.code_start).collect();
    let mut next = 1001;
    while used.contains(&next) {
        next += 1000;
    }
    next
}

/// Faixa pelo nome quando a categoria nova vem sem faixa (regra do `POST /categories` do Node).
fn preferred_code_start(name: &str) -> i64 {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["bar", "cerveja", "chope", "chopp", "drink", "dose", "destilado", "vinho", "whisky", "vodka", "gin"]) {
        5001
    } else if has(&["trident", "chiclete", "bala", "doce", "sobremesa", "tabaco", "cigarro", "conveniencia", "conveniência"]) {
        6001
    } else if has(&["cozinha", "prato", "refeic", "almoco", "jantar"]) {
        1001
    } else if has(&["porcao", "porção", "petisco", "lanche", "burger", "hamburguer", "pastel"]) {
        2001
    } else if has(&["pizza", "massa"]) {
        3001
    } else if has(&["bebida", "refrigerante", "suco", "agua", "água", "energetico", "energético"]) {
        4001
    } else if has(&["alimento", "comida"]) {
        2001
    } else {
        1001
    }
}

/// Próximo código livre da faixa da categoria.
pub async fn next_code(db: &Db, category_id: &str) -> DbResult<String> {
    let mut conn = db.pool().acquire().await?;
    importacao::proximo_codigo_categoria(&mut conn, category_id).await
}

fn category_event() -> Vec<Event> {
    vec![Event::CatalogUpdated { scope: "categories" }]
}

/// Cria a categoria. Sem faixa, escolhe pelo nome (e a próxima livre se já estiver em uso).
pub async fn create_category(db: &Db, name: &str, icon: Option<&str>, code_start: Option<i64>) -> DbResult<(String, Vec<Event>)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DbError::rule("Nome da categoria é obrigatório"));
    }
    let mut tx = db.begin_write().await?;
    let code_start = match code_start.filter(|c| *c > 0) {
        Some(c) => c,
        None => {
            let used: Vec<Option<i64>> = sqlx::query_scalar(r#"SELECT "codeStart" FROM "Category""#).fetch_all(&mut **tx).await?;
            let used: HashSet<i64> = used.into_iter().flatten().collect();
            let preferred = preferred_code_start(name);
            if used.contains(&preferred) {
                let mut c = 1001;
                while used.contains(&c) {
                    c += 1000;
                }
                c
            } else {
                preferred
            }
        }
    };
    let id = ids::cuid();
    sqlx::query(r#"INSERT INTO "Category" ("id", "name", "icon", "sortOrder", "codeStart") VALUES (?, ?, ?, 0, ?)"#)
        .bind(&id)
        .bind(name)
        .bind(icon.filter(|i| !i.is_empty()).unwrap_or("Beer"))
        .bind(code_start)
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    Ok((id, category_event()))
}

/// Altera nome, ícone e faixa. Com `renumber`, renumera os produtos da categoria a partir da faixa.
pub async fn update_category(
    db: &Db,
    id: &str,
    name: &str,
    icon: Option<&str>,
    code_start: Option<i64>,
    renumber: bool,
) -> DbResult<(Option<String>, Vec<Event>)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DbError::rule("Nome da categoria é obrigatório"));
    }
    let mut tx = db.begin_write().await?;
    let changed = sqlx::query(r#"UPDATE "Category" SET "name" = ?, "icon" = COALESCE(?, "icon"), "codeStart" = COALESCE(?, "codeStart") WHERE "id" = ?"#)
        .bind(name)
        .bind(icon.filter(|i| !i.is_empty()))
        .bind(code_start.filter(|c| *c > 0))
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Categoria não encontrada"));
    }
    let message = if renumber { Some(resequence_in(&mut tx, id).await?) } else { None };
    tx.commit().await?;
    let mut events = category_event();
    if renumber {
        events.extend(catalog_event());
    }
    Ok((message, events))
}

async fn resequence_in(conn: &mut SqliteConnection, category_id: &str) -> DbResult<String> {
    let (name, code_start): (String, Option<i64>) =
        sqlx::query_as(r#"SELECT "name", "codeStart" FROM "Category" WHERE "id" = ?"#)
            .bind(category_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| DbError::not_found("Categoria não encontrada"))?;
    let base = code_start.filter(|c| *c > 0).unwrap_or(1001);
    let others: Vec<Option<String>> = sqlx::query_scalar(r#"SELECT "code" FROM "Product" WHERE "categoryId" <> ?"#)
        .bind(category_id)
        .fetch_all(&mut *conn)
        .await?;
    let mut used: HashSet<String> = others.into_iter().flatten().map(|c| c.trim().to_string()).collect();
    let products: Vec<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Product" WHERE "categoryId" = ? ORDER BY "name""#)
        .bind(category_id)
        .fetch_all(&mut *conn)
        .await?;
    let mut current = base;
    for id in &products {
        while used.contains(&current.to_string()) {
            current += 1;
        }
        used.insert(current.to_string());
        set_code(conn, id, &current.to_string()).await?;
        current += 1;
    }
    Ok(format!(
        r#"✅ {} produtos da categoria "{name}" renumerados sequencialmente a partir de #{base}."#,
        products.len()
    ))
}

async fn set_code(conn: &mut SqliteConnection, id: &str, code: &str) -> DbResult<()> {
    sqlx::query(r#"UPDATE "Product" SET "code" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(code)
        .bind(DbTime::now())
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Renumera os produtos da categoria a partir da faixa dela, sem colidir com as outras.
pub async fn resequence_category(db: &Db, id: &str) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let message = resequence_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok((message, catalog_event()))
}

/// Exclui a categoria e os produtos dela nunca vendidos. Recusa se houver produto já vendido.
pub async fn delete_category(db: &Db, id: &str) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let name: String = sqlx::query_scalar(r#"SELECT "name" FROM "Category" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::not_found("Categoria não encontrada"))?;
    let sold: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM "Product" p WHERE p."categoryId" = ?
             AND EXISTS (SELECT 1 FROM "OrderItem" i WHERE i."productId" = p."id")"#,
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    if sold > 0 {
        return Err(DbError::rule(format!(
            r#"A categoria "{name}" tem {sold} produto(s) que já foram vendidos. Mova esses produtos para outra categoria antes de excluir (o histórico de vendas precisa deles)."#
        )));
    }
    let products: Vec<(String, String)> = sqlx::query_as(r#"SELECT "id", "name" FROM "Product" WHERE "categoryId" = ?"#)
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    for (pid, pname) in &products {
        remove_product(&mut tx, pid, pname).await?;
    }
    sqlx::query(r#"DELETE FROM "Category" WHERE "id" = ?"#).bind(id).execute(&mut **tx).await?;
    tx.commit().await?;
    let mut events = category_event();
    events.extend(catalog_event());
    Ok((format!(r#"Categoria "{name}" excluída com sucesso!"#), events))
}

/// Dá código sequencial a todos os produtos sem código. Devolve a mensagem.
pub async fn backfill_codes(db: &Db) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let missing: Vec<(String, String)> = sqlx::query_as(
        r#"SELECT "id", "categoryId" FROM "Product" WHERE "code" IS NULL OR trim("code") = '' ORDER BY "name""#,
    )
    .fetch_all(&mut **tx)
    .await?;
    if missing.is_empty() {
        return Ok(("Todos os produtos já possuem código interno.".into(), Vec::new()));
    }
    for (id, category_id) in &missing {
        let code = importacao::proximo_codigo_categoria(&mut tx, category_id).await?;
        set_code(&mut tx, id, &code).await?;
    }
    tx.commit().await?;
    Ok((format!("✅ {} produtos receberam código interno sequencial.", missing.len()), catalog_event()))
}

/// Corrige códigos repetidos ou vazios (`all = false`) ou renumera o cardápio inteiro por categoria (`all = true`).
pub async fn fix_duplicate_codes(db: &Db, all: bool) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let cats: Vec<(String, Option<i64>)> =
        sqlx::query_as(r#"SELECT "id", "codeStart" FROM "Category" ORDER BY "sortOrder""#).fetch_all(&mut **tx).await?;
    let base_of: HashMap<String, i64> =
        cats.iter().map(|(id, c)| (id.clone(), c.filter(|c| *c > 0).unwrap_or(1001))).collect();

    if all {
        let mut used: HashSet<String> = HashSet::new();
        let mut total = 0;
        for (cat_id, _) in &cats {
            let products: Vec<String> =
                sqlx::query_scalar(r#"SELECT "id" FROM "Product" WHERE "categoryId" = ? ORDER BY "name""#)
                    .bind(cat_id)
                    .fetch_all(&mut **tx)
                    .await?;
            let mut n = base_of[cat_id];
            for id in products {
                while used.contains(&n.to_string()) {
                    n += 1;
                }
                used.insert(n.to_string());
                set_code(&mut tx, &id, &n.to_string()).await?;
                total += 1;
                n += 1;
            }
        }
        tx.commit().await?;
        return Ok((
            format!("✅ Todo o cardápio foi renumerado por categoria com códigos exclusivos ({total} produtos)."),
            catalog_event(),
        ));
    }

    let products: Vec<(String, Option<String>, String)> = sqlx::query_as(
        r#"SELECT "id", "code", "categoryId" FROM "Product" ORDER BY "categoryId", "name""#,
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut kept: HashSet<String> = HashSet::new();
    let mut needs: Vec<(String, String)> = Vec::new();
    let mut missing: Vec<(String, String)> = Vec::new();
    for (id, code, cat) in products {
        match code.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) {
            None => missing.push((id, cat)),
            Some(c) if kept.contains(&c) => needs.push((id, cat)),
            Some(c) => {
                kept.insert(c);
            }
        }
    }
    needs.extend(missing);
    if needs.is_empty() {
        return Ok(("Nenhum código duplicado ou em branco foi encontrado. O cardápio já está 100% íntegro!".into(), Vec::new()));
    }
    for (id, cat) in &needs {
        let mut candidate = base_of.get(cat).copied().unwrap_or(1001);
        while kept.contains(&candidate.to_string()) {
            candidate += 1;
        }
        kept.insert(candidate.to_string());
        set_code(&mut tx, id, &candidate.to_string()).await?;
    }
    tx.commit().await?;
    Ok((
        format!("✅ {} produto(s) com numeração duplicada ou ausente foram corrigidos com códigos únicos!", needs.len()),
        catalog_event(),
    ))
}

// ============================================================
// Organizar Bar / Cozinha
// ============================================================

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Número seguido de uma das unidades (`\b\d+(?:[.,]\d+)?\s*(?:unidades)\b`, como a regex do Node).
fn has_number_with_unit(text: &str, units: &[&str]) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() || (i > 0 && is_word(chars[i - 1])) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        let mut ends = vec![j];
        if j + 1 < chars.len() && (chars[j] == '.' || chars[j] == ',') && chars[j + 1].is_ascii_digit() {
            let mut k = j + 1;
            while k < chars.len() && chars[k].is_ascii_digit() {
                k += 1;
            }
            ends.push(k);
        }
        for end in ends {
            let mut k = end;
            while k < chars.len() && chars[k].is_whitespace() {
                k += 1;
            }
            let rest: String = chars[k..].iter().collect();
            for u in units {
                if let Some(after) = rest.strip_prefix(u) {
                    let next = after.chars().next();
                    // `\b` depois da unidade: só conta se a unidade termina em letra e o próximo não é letra/dígito
                    let unit_ends_in_word = u.chars().last().is_some_and(is_word);
                    let boundary = if unit_ends_in_word { next.is_none_or(|c| !is_word(c)) } else { next.is_some_and(is_word) };
                    if boundary {
                        return true;
                    }
                }
            }
        }
        i = j.max(i + 1);
    }
    false
}

/// Nome com volume em ml ou litros (a tela manda esses itens para o Bar ao digitar o nome).
pub fn has_volume(name: &str) -> bool {
    let t = name.to_lowercase();
    has_number_with_unit(&t, &["ml", "m.l."]) || has_number_with_unit(&t, &["litros", "litro", "lt", "l"])
}

/// É bebida/item de bar? Tradução de `isBarProduct` (`server/src/routes/importXml.ts`).
pub fn is_bar_product(name: &str, description: Option<&str>, unit: Option<&str>) -> bool {
    let text = format!("{} {}", name, description.unwrap_or("")).to_lowercase();
    let u = unit.unwrap_or("").trim().to_uppercase();
    if has_number_with_unit(&text, &["ml", "m.l."]) {
        return true;
    }
    if matches!(u.as_str(), "ML" | "L" | "LT" | "LTS") {
        return true;
    }
    if has_number_with_unit(&text, &["litros", "litro", "lt", "l"]) {
        return true;
    }
    const ANYWHERE: [&str; 38] = [
        "cervej", "chopp", "chope", "beer", "refrigerante", "coca-cola", "coca cola", "cocacola", "pepsi", "guarana",
        "guaraná", "fanta", "sprite", "schweppes", "vodka", "whisky", "whiskey", "cachaca", "cachaça", "pinga",
        "tequila", "licor", "vinho", "espumante", "champagne", "suco", "energetico", "energético", "red bull",
        "monster", "água", "agua mineral", "long neck", "latao", "latão", "campari", "aperol", "conhaque",
    ];
    const MORE: [&str; 6] = ["sake", "saque", "saquê", "destilado", "coquetel", "drink"];
    if ANYWHERE.iter().chain(MORE.iter()).any(|w| text.contains(w)) {
        return true;
    }
    // Termos com `\b` no fim na regex do Node
    for w in ["gin", "rum", "ice", "dose"] {
        let mut start = 0;
        while let Some(pos) = text[start..].find(w) {
            let end = start + pos + w.len();
            if text[end..].chars().next().is_none_or(|c| !is_word(c)) {
                return true;
            }
            start = end;
        }
    }
    false
}

/// id, nome, descrição, unidade, categoria, destino, nome da categoria
type OrganizeRow = (String, String, Option<String>, String, String, String, Option<String>);

async fn category_named(conn: &mut SqliteConnection, names: &[&str]) -> DbResult<Option<String>> {
    for n in names {
        let id: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Category" WHERE "name" = ? LIMIT 1"#)
            .bind(n)
            .fetch_optional(&mut *conn)
            .await?;
        if id.is_some() {
            return Ok(id);
        }
    }
    Ok(None)
}

async fn ensure_category(conn: &mut SqliteConnection, names: &[&str], icon: &str, sort: i64, code_start: i64) -> DbResult<String> {
    if let Some(id) = category_named(conn, names).await? {
        return Ok(id);
    }
    let id = ids::cuid();
    sqlx::query(r#"INSERT INTO "Category" ("id", "name", "icon", "sortOrder", "codeStart") VALUES (?, ?, ?, ?, ?)"#)
        .bind(&id)
        .bind(names[0])
        .bind(icon)
        .bind(sort)
        .bind(code_start)
        .execute(&mut *conn)
        .await?;
    Ok(id)
}

/// Bebidas para o Bar e o que veio em "IMPORTADOS" para a Cozinha (botão "Organizar Bar/Cozinha").
pub async fn organize_bar_kitchen(db: &Db) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let bar = ensure_category(&mut tx, &["Bar", "BAR", "bar"], "Beer", 1, 5001).await?;
    let kitchen = ensure_category(&mut tx, &["Cozinha", "COZINHA", "cozinha"], "UtensilsCrossed", 2, 1001).await?;
    let products: Vec<OrganizeRow> = sqlx::query_as(
        r#"SELECT p."id", p."name", p."description", p."unit", p."categoryId", p."kdsStation", c."name"
           FROM "Product" p LEFT JOIN "Category" c ON c."id" = p."categoryId""#,
    )
    .fetch_all(&mut **tx)
    .await?;
    let (mut bar_count, mut kitchen_count) = (0, 0);
    for (id, name, description, unit, category_id, station, cat_name) in products {
        let imported = cat_name.unwrap_or_default().to_uppercase().contains("IMPORTAD");
        let is_bar = is_bar_product(&name, description.as_deref(), Some(&unit));
        if imported || (is_bar && (category_id == kitchen || station != "BAR")) {
            let (new_cat, new_station) = if is_bar {
                (bar.clone(), "BAR".to_string())
            } else if imported {
                (kitchen.clone(), "KITCHEN".to_string())
            } else {
                (category_id.clone(), station.clone())
            };
            sqlx::query(r#"UPDATE "Product" SET "categoryId" = ?, "kdsStation" = ?, "updatedAt" = ? WHERE "id" = ?"#)
                .bind(new_cat)
                .bind(new_station)
                .bind(DbTime::now())
                .bind(&id)
                .execute(&mut **tx)
                .await?;
            if is_bar {
                bar_count += 1;
            } else if imported {
                kitchen_count += 1;
            }
        }
    }
    if let Some(imported) = category_named(&mut tx, &["IMPORTADOS", "Importados", "importados"]).await? {
        let left: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "Product" WHERE "categoryId" = ?"#)
            .bind(&imported)
            .fetch_one(&mut **tx)
            .await?;
        if left == 0 {
            sqlx::query(r#"DELETE FROM "Category" WHERE "id" = ?"#).bind(&imported).execute(&mut **tx).await?;
        }
    }
    tx.commit().await?;
    let mut events = catalog_event();
    events.extend(category_event());
    Ok((format!("Organização concluída: {bar_count} produtos no Bar e {kitchen_count} na Cozinha."), events))
}

// ============================================================
// Relatório de estoque & lucro previsto
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockReportCategory {
    pub category: String,
    /// Produtos ativos na categoria
    pub items_count: i64,
    /// Unidades em estoque
    pub units_count: i64,
    pub total_cost: f64,
    pub total_sale: f64,
    pub expected_profit: f64,
    pub margin_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockReport {
    pub total_products: i64,
    pub total_stock_units: i64,
    pub total_cost_value: f64,
    pub total_sale_value: f64,
    pub total_expected_profit: f64,
    pub average_margin: f64,
    pub categories: Vec<StockReportCategory>,
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Valor do estoque a custo e a venda e o lucro previsto, no total e por categoria (estoque negativo conta 0).
pub async fn stock_report(db: &Db) -> DbResult<StockReport> {
    let cats: Vec<(String, String)> =
        sqlx::query_as(r#"SELECT "id", "name" FROM "Category" ORDER BY "sortOrder", "name""#).fetch_all(db.pool()).await?;
    let products: Vec<(String, i64, Option<f64>, f64)> = sqlx::query_as(
        r#"SELECT "categoryId", CAST("stock" AS INTEGER), "costPrice", "price" FROM "Product" WHERE "isActive" = 1"#,
    )
    .fetch_all(db.pool())
    .await?;
    let mut by_cat: HashMap<String, StockReportCategory> = cats
        .iter()
        .map(|(id, name)| {
            (
                id.clone(),
                StockReportCategory {
                    category: name.clone(),
                    items_count: 0,
                    units_count: 0,
                    total_cost: 0.0,
                    total_sale: 0.0,
                    expected_profit: 0.0,
                    margin_percent: 0.0,
                },
            )
        })
        .collect();
    let (mut units, mut cost_total, mut sale_total) = (0i64, 0.0, 0.0);
    for (cat, stock, cost, price) in &products {
        let stock = (*stock).max(0);
        let cost = round2(stock as f64 * cost.unwrap_or(0.0));
        let sale = round2(stock as f64 * price);
        units += stock;
        cost_total += cost;
        sale_total += sale;
        if let Some(c) = by_cat.get_mut(cat) {
            c.items_count += 1;
            c.units_count += stock;
            c.total_cost = round2(c.total_cost + cost);
            c.total_sale = round2(c.total_sale + sale);
            c.expected_profit = round2(c.total_sale - c.total_cost);
        }
    }
    let categories = cats
        .iter()
        .filter_map(|(id, _)| by_cat.remove(id))
        .map(|mut c| {
            c.margin_percent = if c.total_sale > 0.0 { round1((c.total_sale - c.total_cost) / c.total_sale * 100.0) } else { 0.0 };
            c
        })
        .collect();
    Ok(StockReport {
        total_products: products.len() as i64,
        total_stock_units: units,
        total_cost_value: round2(cost_total),
        total_sale_value: round2(sale_total),
        total_expected_profit: round2(sale_total - cost_total),
        average_margin: if sale_total > 0.0 { round1((sale_total - cost_total) / sale_total * 100.0) } else { 0.0 },
        categories,
    })
}

#[cfg(test)]
#[path = "products_tests.rs"]
mod tests;
