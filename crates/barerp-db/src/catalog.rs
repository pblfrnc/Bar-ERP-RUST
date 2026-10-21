//! Produtos à venda, categorias, adicionais e promoções — o que a tela de lançamento precisa.

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use barerp_core::pricing::{ModifierGroup, ModifierOption, Promotion};

use crate::{Db, DbResult};

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    #[sqlx(rename = "sortOrder")]
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SaleProduct {
    pub id: String,
    pub code: Option<String>,
    pub ean: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub price: f64,
    #[sqlx(rename = "categoryId")]
    pub category_id: String,
    #[sqlx(rename = "kdsStation")]
    pub kds_station: String,
    pub stock: i64,
    #[sqlx(rename = "trackStock")]
    pub track_stock: bool,
    pub unit: String,
    #[sqlx(rename = "hasBoxPrice")]
    pub has_box_price: bool,
    #[sqlx(rename = "boxQuantity")]
    pub box_quantity: Option<i64>,
    #[sqlx(rename = "boxPrice")]
    pub box_price: Option<f64>,
    #[sqlx(rename = "boxEan")]
    pub box_ean: Option<String>,
    /// Tem pergunta/adicional ativo (abre a escolha de opções ao lançar)
    #[sqlx(rename = "hasModifiers")]
    pub has_modifiers: bool,
}

pub async fn categories(db: &Db) -> DbResult<Vec<Category>> {
    Ok(sqlx::query_as(r#"SELECT "id", "name", "icon", "sortOrder" FROM "Category" ORDER BY "sortOrder", "name""#)
        .fetch_all(db.pool())
        .await?)
}

/// Produtos ativos, em ordem alfabética.
pub async fn sale_products(db: &Db) -> DbResult<Vec<SaleProduct>> {
    Ok(sqlx::query_as(
        r#"SELECT p."id", p."code", p."ean", p."name", p."description", p."price", p."categoryId", p."kdsStation", p."stock",
                  p."trackStock", p."unit", p."hasBoxPrice", p."boxQuantity", p."boxPrice", p."boxEan",
                  EXISTS (SELECT 1 FROM "ProductModifierGroup" l JOIN "ModifierGroup" g ON g."id" = l."groupId"
                          WHERE l."productId" = p."id" AND g."isActive" = 1) AS "hasModifiers"
           FROM "Product" p
           WHERE p."isActive" = 1
           ORDER BY p."name" COLLATE NOCASE"#,
    )
    .fetch_all(db.pool())
    .await?)
}

#[derive(FromRow)]
struct PromotionRow {
    id: String,
    name: String,
    #[sqlx(rename = "isActive")]
    is_active: bool,
    #[sqlx(rename = "daysOfWeek")]
    days_of_week: String,
    #[sqlx(rename = "startTime")]
    start_time: String,
    #[sqlx(rename = "endTime")]
    end_time: String,
    #[sqlx(rename = "discountType")]
    discount_type: String,
    value: f64,
    #[sqlx(rename = "productIds")]
    product_ids: String,
    #[sqlx(rename = "categoryIds")]
    category_ids: String,
}

pub async fn active_promotions(conn: &mut SqliteConnection) -> DbResult<Vec<Promotion>> {
    let rows: Vec<PromotionRow> = sqlx::query_as(
        r#"SELECT "id", "name", "isActive", "daysOfWeek", "startTime", "endTime", "discountType", "value",
                  "productIds", "categoryIds"
           FROM "Promotion" WHERE "isActive" = 1"#,
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Promotion {
            id: r.id,
            name: r.name,
            is_active: r.is_active,
            days_of_week: r.days_of_week,
            start_time: r.start_time,
            end_time: r.end_time,
            discount_type: r.discount_type,
            value: r.value,
            product_ids: r.product_ids,
            category_ids: r.category_ids,
        })
        .collect())
}

/// Grupos de adicionais ativos do produto, na ordem do produto, só com as opções ativas.
pub async fn modifier_groups(conn: &mut SqliteConnection, product_id: &str) -> DbResult<Vec<(String, ModifierGroup)>> {
    let groups: Vec<(String, String, i64, i64)> = sqlx::query_as(
        r#"SELECT g."id", g."name", g."minSelect", g."maxSelect"
           FROM "ProductModifierGroup" l JOIN "ModifierGroup" g ON g."id" = l."groupId"
           WHERE l."productId" = ? AND g."isActive" = 1
           ORDER BY l."sortOrder""#,
    )
    .bind(product_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut result = Vec::with_capacity(groups.len());
    for (id, name, min_select, max_select) in groups {
        let options: Vec<(String, String, f64)> = sqlx::query_as(
            r#"SELECT "id", "name", "price" FROM "ModifierOption"
               WHERE "groupId" = ? AND "isActive" = 1 ORDER BY "sortOrder", "name""#,
        )
        .bind(&id)
        .fetch_all(&mut *conn)
        .await?;
        let options = options.into_iter().map(|(id, name, price)| ModifierOption { id, name, price }).collect();
        result.push((id, ModifierGroup { name, min_select, max_select, options }));
    }
    Ok(result)
}

/// Grupos de adicionais do produto para a tela de escolha (com o id de cada grupo).
pub async fn product_modifiers(db: &Db, product_id: &str) -> DbResult<Vec<(String, ModifierGroup)>> {
    let mut conn = db.pool().acquire().await?;
    modifier_groups(&mut conn, product_id).await
}

/// Promoções ativas (para a tela mostrar o preço que o servidor vai cobrar).
pub async fn promotions(db: &Db) -> DbResult<Vec<Promotion>> {
    let mut conn = db.pool().acquire().await?;
    active_promotions(&mut conn).await
}
