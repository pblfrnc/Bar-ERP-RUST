//! Cardápio digital: linha `DigitalMenuSettings`, dados públicos do cardápio e fotos dos produtos
//! (`ProductImage`). Tradução das consultas de `server/src/services/digitalMenu.ts` e das rotas
//! `/products/:id/image` e `/products/menu-visibility` de `server/src/routes/products.ts`.
//!
//! A montagem e a publicação ficam no crate `barerp-menu-publish` (adaptador `db_store`).

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use crate::{Db, DbError, DbResult, DbTime};

/// Limite da foto (o mesmo do Node e da função do Netlify).
pub const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct DigitalMenuRow {
    pub enabled: bool,
    #[sqlx(rename = "siteUrl")]
    pub site_url: Option<String>,
    #[sqlx(rename = "storeId")]
    pub store_id: Option<String>,
    #[sqlx(rename = "publishSecret")]
    pub publish_secret: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    #[sqlx(rename = "hiddenCategoryIds")]
    pub hidden_category_ids: String,
    #[sqlx(rename = "hideSoldOut")]
    pub hide_sold_out: bool,
    #[sqlx(rename = "ignoreStock")]
    pub ignore_stock: bool,
    #[sqlx(rename = "lastPublishedAt")]
    pub last_published_at: Option<DbTime>,
    #[sqlx(rename = "lastHash")]
    pub last_hash: Option<String>,
    #[sqlx(rename = "lastError")]
    pub last_error: Option<String>,
}

const SETTINGS_SELECT: &str = r#"SELECT "enabled", "siteUrl", "storeId", "publishSecret", "title", "message",
       "hiddenCategoryIds", "hideSoldOut", "ignoreStock", "lastPublishedAt", "lastHash", "lastError"
FROM "DigitalMenuSettings" WHERE "id" = 'default'"#;

/// Linha `id = 'default'`, criada com os padrões se ainda não existir (o `upsert` do Node).
pub async fn settings(db: &Db) -> DbResult<DigitalMenuRow> {
    if let Some(row) = sqlx::query_as(SETTINGS_SELECT)
        .fetch_optional(db.pool())
        .await?
    {
        return Ok(row);
    }
    let mut tx = db.begin_write().await?;
    sqlx::query(r#"INSERT INTO "DigitalMenuSettings" ("id", "updatedAt") VALUES ('default', ?) ON CONFLICT DO NOTHING"#)
        .bind(DbTime::now())
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query_as(SETTINGS_SELECT).fetch_one(&mut **tx).await?;
    tx.commit().await?;
    Ok(row)
}

/// Nome fantasia (fiscal) e nome do restaurante, para o título padrão do cardápio.
pub async fn title_sources(db: &Db) -> DbResult<(Option<String>, Option<String>)> {
    let fantasia: Option<Option<String>> =
        sqlx::query_scalar(r#"SELECT "nomeFantasia" FROM "FiscalSettings" WHERE "id" = 'default'"#)
            .fetch_optional(db.pool())
            .await?;
    let restaurant: Option<String> = sqlx::query_scalar(
        r#"SELECT "restaurantName" FROM "SystemSettings" WHERE "id" = 'default'"#,
    )
    .fetch_optional(db.pool())
    .await?;
    Ok((fantasia.flatten(), restaurant))
}

/// Campos que a tela de configurações grava. Sempre limpa `lastError`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DigitalMenuUpdate {
    pub site_url: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub hidden_category_ids: String,
    pub enabled: bool,
    pub hide_sold_out: bool,
    pub ignore_stock: bool,
    /// Também zera `lastHash` (trocou de site ou desligou)
    pub clear_last_hash: bool,
}

/// Garante a linha e roda a gravação na mesma transação.
async fn write_settings(
    db: &Db,
    sql: &str,
    binds: impl FnOnce(SqlQuery<'_>) -> SqlQuery<'_>,
) -> DbResult<()> {
    settings(db).await?;
    let mut tx = db.begin_write().await?;
    binds(sqlx::query(sql)).execute(&mut **tx).await?;
    tx.commit().await?;
    Ok(())
}

type SqlQuery<'q> = sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>;

pub async fn update_settings(db: &Db, u: DigitalMenuUpdate) -> DbResult<()> {
    write_settings(
        db,
        r#"UPDATE "DigitalMenuSettings" SET "siteUrl" = ?, "title" = ?, "message" = ?, "hiddenCategoryIds" = ?,
               "enabled" = ?, "hideSoldOut" = ?, "ignoreStock" = ?, "lastError" = NULL,
               "lastHash" = CASE WHEN ? THEN NULL ELSE "lastHash" END, "updatedAt" = ?
           WHERE "id" = 'default'"#,
        |q| {
            q.bind(u.site_url)
                .bind(u.title)
                .bind(u.message)
                .bind(u.hidden_category_ids)
                .bind(u.enabled)
                .bind(u.hide_sold_out)
                .bind(u.ignore_stock)
                .bind(u.clear_last_hash)
                .bind(DbTime::now())
        },
    )
    .await
}

/// Loja nova no site: grava `storeId` e `publishSecret` e zera `lastHash`.
pub async fn set_credentials(db: &Db, store_id: &str, secret: &str) -> DbResult<()> {
    let (store_id, secret) = (store_id.to_string(), secret.to_string());
    write_settings(
        db,
        r#"UPDATE "DigitalMenuSettings" SET "storeId" = ?, "publishSecret" = ?, "lastHash" = NULL, "updatedAt" = ?
           WHERE "id" = 'default'"#,
        |q| q.bind(store_id).bind(secret).bind(DbTime::now()),
    )
    .await
}

/// Publicou: grava o hash (ou `NULL` para tentar de novo), a hora e limpa o erro.
pub async fn record_published(db: &Db, last_hash: Option<&str>, at: DbTime) -> DbResult<()> {
    let last_hash = last_hash.map(str::to_string);
    write_settings(
        db,
        r#"UPDATE "DigitalMenuSettings" SET "lastHash" = ?, "lastPublishedAt" = ?, "lastError" = NULL, "updatedAt" = ?
           WHERE "id" = 'default'"#,
        |q| q.bind(last_hash).bind(at).bind(DbTime::now()),
    )
    .await
}

pub async fn record_error(db: &Db, message: &str) -> DbResult<()> {
    let message = message.to_string();
    write_settings(
        db,
        r#"UPDATE "DigitalMenuSettings" SET "lastError" = ?, "updatedAt" = ? WHERE "id" = 'default'"#,
        |q| q.bind(message).bind(DbTime::now()),
    )
    .await
}

// ---------- Dados do cardápio ----------

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct MenuCategoryRow {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    #[sqlx(rename = "sortOrder")]
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct MenuProductRow {
    pub id: String,
    #[sqlx(rename = "categoryId")]
    pub category_id: String,
    pub name: String,
    pub description: Option<String>,
    pub price: f64,
    #[sqlx(rename = "isActive")]
    pub is_active: bool,
    #[sqlx(rename = "showInMenu")]
    pub show_in_menu: bool,
    #[sqlx(rename = "kdsStation")]
    pub kds_station: String,
    #[sqlx(rename = "trackStock")]
    pub track_stock: bool,
    pub stock: i64,
    #[sqlx(rename = "costPrice")]
    pub cost_price: Option<f64>,
    #[sqlx(rename = "hasBoxPrice")]
    pub has_box_price: bool,
    #[sqlx(rename = "boxPrice")]
    pub box_price: Option<f64>,
    #[sqlx(rename = "boxQuantity")]
    pub box_quantity: Option<i64>,
    #[sqlx(rename = "boxCostPrice")]
    pub box_cost_price: Option<f64>,
}

/// Grupo de adicionais ligado ao produto (`ProductModifierGroup` + `ModifierGroup`).
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct MenuGroupLinkRow {
    #[sqlx(rename = "productId")]
    pub product_id: String,
    #[sqlx(rename = "linkOrder")]
    pub link_order: i64,
    #[sqlx(rename = "groupId")]
    pub group_id: String,
    pub name: String,
    #[sqlx(rename = "isActive")]
    pub is_active: bool,
    #[sqlx(rename = "minSelect")]
    pub min_select: i64,
    #[sqlx(rename = "maxSelect")]
    pub max_select: i64,
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct MenuOptionRow {
    #[sqlx(rename = "groupId")]
    pub group_id: String,
    pub name: String,
    pub price: f64,
    #[sqlx(rename = "isActive")]
    pub is_active: bool,
    #[sqlx(rename = "sortOrder")]
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct MenuPromotionRow {
    pub id: String,
    pub name: String,
    #[sqlx(rename = "isActive")]
    pub is_active: bool,
    #[sqlx(rename = "daysOfWeek")]
    pub days_of_week: String,
    #[sqlx(rename = "startTime")]
    pub start_time: String,
    #[sqlx(rename = "endTime")]
    pub end_time: String,
    #[sqlx(rename = "discountType")]
    pub discount_type: String,
    pub value: f64,
    #[sqlx(rename = "productIds")]
    pub product_ids: String,
    #[sqlx(rename = "categoryIds")]
    pub category_ids: String,
}

/// Tudo que o cardápio publicado usa, já com os filtros das consultas do Node (produtos ativos, no
/// cardápio e com preço; promoções ativas). A ordem final é aplicada em `barerp_menu_publish::build_menu`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuData {
    pub categories: Vec<MenuCategoryRow>,
    pub products: Vec<MenuProductRow>,
    pub group_links: Vec<MenuGroupLinkRow>,
    pub options: Vec<MenuOptionRow>,
    pub promotions: Vec<MenuPromotionRow>,
    /// (productId, hash)
    pub image_hashes: Vec<(String, String)>,
}

pub async fn menu_data(db: &Db) -> DbResult<MenuData> {
    let mut conn = db.pool().acquire().await?;
    let conn: &mut SqliteConnection = &mut conn;
    let categories = sqlx::query_as(r#"SELECT "id", "name", "icon", "sortOrder" FROM "Category""#)
        .fetch_all(&mut *conn)
        .await?;
    let products = sqlx::query_as(
        r#"SELECT "id", "categoryId", "name", "description", "price", "isActive", "showInMenu", "kdsStation",
                  "trackStock", CAST("stock" AS INTEGER) AS "stock", "costPrice", "hasBoxPrice", "boxPrice",
                  "boxQuantity", "boxCostPrice"
           FROM "Product" WHERE "isActive" = 1 AND "showInMenu" = 1 AND "price" > 0"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    let group_links = sqlx::query_as(
        r#"SELECT l."productId", l."sortOrder" AS "linkOrder", g."id" AS "groupId", g."name", g."isActive",
                  g."minSelect", g."maxSelect"
           FROM "ProductModifierGroup" l JOIN "ModifierGroup" g ON g."id" = l."groupId"
           JOIN "Product" p ON p."id" = l."productId"
           WHERE p."isActive" = 1 AND p."showInMenu" = 1 AND p."price" > 0"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    let options = sqlx::query_as(
        r#"SELECT "groupId", "name", "price", "isActive", "sortOrder" FROM "ModifierOption" WHERE "isActive" = 1"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    let promotions = sqlx::query_as(
        r#"SELECT "id", "name", "isActive", "daysOfWeek", "startTime", "endTime", "discountType", "value",
                  "productIds", "categoryIds"
           FROM "Promotion" WHERE "isActive" = 1"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    let image_hashes = sqlx::query_as(r#"SELECT "productId", "hash" FROM "ProductImage""#)
        .fetch_all(&mut *conn)
        .await?;
    Ok(MenuData {
        categories,
        products,
        group_links,
        options,
        promotions,
        image_hashes,
    })
}

/// Produtos que aparecem (`show`) e que saem (`hide`) do cardápio digital (`PUT /products/menu-visibility`).
pub async fn set_menu_visibility(db: &Db, show: &[String], hide: &[String]) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    for (ids, visible) in [(show, true), (hide, false)] {
        for id in ids {
            sqlx::query(r#"UPDATE "Product" SET "showInMenu" = ?, "updatedAt" = ? WHERE "id" = ?"#)
                .bind(visible)
                .bind(DbTime::now())
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

// ---------- Fotos ----------

/// Foto e o hash que o site já tem.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct ImageRefRow {
    #[sqlx(rename = "productId")]
    pub product_id: String,
    pub hash: String,
    #[sqlx(rename = "publishedHash")]
    pub published_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ProductImage {
    #[serde(skip)]
    pub data: Vec<u8>,
    #[sqlx(rename = "mimeType")]
    pub mime_type: String,
    pub hash: String,
}

/// Tipo pela assinatura do arquivo (não confia no Content-Type): JPEG, PNG ou WebP.
pub fn detect_image_mime(data: &[u8]) -> Option<&'static str> {
    if data.len() > 3 && data[..3] == [0xFF, 0xD8, 0xFF] {
        Some("image/jpeg")
    } else if data.len() > 8 && data[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        Some("image/png")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub async fn images(db: &Db, product_ids: &[String]) -> DbResult<Vec<ImageRefRow>> {
    let mut out = Vec::new();
    for id in product_ids {
        let row: Option<ImageRefRow> =
            sqlx::query_as(r#"SELECT "productId", "hash", "publishedHash" FROM "ProductImage" WHERE "productId" = ?"#)
                .bind(id)
                .fetch_optional(db.pool())
                .await?;
        out.extend(row);
    }
    Ok(out)
}

/// `GET /products/:id/image`
pub async fn product_image(db: &Db, product_id: &str) -> DbResult<Option<ProductImage>> {
    Ok(sqlx::query_as(
        r#"SELECT "data", "mimeType", "hash" FROM "ProductImage" WHERE "productId" = ?"#,
    )
    .bind(product_id)
    .fetch_optional(db.pool())
    .await?)
}

/// `PUT /products/:id/image`: confere tamanho e formato e grava. `hash` é o
/// `barerp_menu_publish::image_hash(&data)` (o adaptador `save_product_image` já calcula).
pub async fn put_product_image(
    db: &Db,
    product_id: &str,
    data: Vec<u8>,
    hash: &str,
) -> DbResult<()> {
    if data.is_empty() {
        return Err(DbError::rule("Envie a foto (JPEG, PNG ou WebP)."));
    }
    if data.len() > MAX_IMAGE_BYTES {
        return Err(DbError::rule("Foto grande demais (máximo 2 MB)."));
    }
    let mime = detect_image_mime(&data)
        .ok_or_else(|| DbError::rule("Formato de imagem não suportado. Use JPEG, PNG ou WebP."))?;
    let mut tx = db.begin_write().await?;
    let exists: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Product" WHERE "id" = ?"#)
        .bind(product_id)
        .fetch_optional(&mut **tx)
        .await?;
    if exists.is_none() {
        return Err(DbError::not_found("Produto não encontrado"));
    }
    sqlx::query(
        r#"INSERT INTO "ProductImage" ("productId", "data", "mimeType", "hash", "updatedAt") VALUES (?, ?, ?, ?, ?)
           ON CONFLICT ("productId") DO UPDATE SET "data" = excluded."data", "mimeType" = excluded."mimeType",
                                                   "hash" = excluded."hash", "updatedAt" = excluded."updatedAt""#,
    )
    .bind(product_id)
    .bind(data)
    .bind(mime)
    .bind(hash)
    .bind(DbTime::now())
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// `DELETE /products/:id/image` (sem foto também é sucesso, como o `deleteMany`).
pub async fn delete_product_image(db: &Db, product_id: &str) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    sqlx::query(r#"DELETE FROM "ProductImage" WHERE "productId" = ?"#)
        .bind(product_id)
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn mark_image_published(db: &Db, product_id: &str, hash: &str) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"UPDATE "ProductImage" SET "publishedHash" = ?, "updatedAt" = ? WHERE "productId" = ?"#,
    )
    .bind(hash)
    .bind(DbTime::now())
    .bind(product_id)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Site novo ou loja nova: todas as fotos precisam subir de novo.
pub async fn reset_published_images(db: &Db) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    sqlx::query(r#"UPDATE "ProductImage" SET "publishedHash" = NULL"#)
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use barerp_core::ids;

    const PNG: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    async fn product(db: &Db) -> String {
        let (cat, id) = (ids::cuid(), ids::cuid());
        let n = DbTime::now();
        sqlx::query(r#"INSERT INTO "Category" ("id","name") VALUES (?, 'Bebidas')"#)
            .bind(&cat)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(r#"INSERT INTO "Product" ("id","name","price","categoryId","createdAt","updatedAt") VALUES (?, 'Chopp', 10, ?, ?, ?)"#)
            .bind(&id)
            .bind(&cat)
            .bind(n)
            .bind(n)
            .execute(db.pool())
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn configuracoes_criadas_e_gravadas() {
        let db = Db::open_in_memory().await.unwrap();
        let s = settings(&db).await.unwrap();
        assert!(!s.enabled && s.store_id.is_none());
        assert_eq!(s.hidden_category_ids, "[]");

        set_credentials(&db, "abc", "segredo").await.unwrap();
        record_published(&db, Some("h1"), DbTime::now())
            .await
            .unwrap();
        record_error(&db, "sem internet").await.unwrap();
        let s = settings(&db).await.unwrap();
        assert_eq!(
            (s.store_id.as_deref(), s.last_hash.as_deref()),
            (Some("abc"), Some("h1"))
        );
        assert_eq!(s.last_error.as_deref(), Some("sem internet"));
        assert!(s.last_published_at.is_some());

        update_settings(
            &db,
            DigitalMenuUpdate {
                enabled: true,
                title: Some("Bar".into()),
                hidden_category_ids: r#"["x"]"#.into(),
                clear_last_hash: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let s = settings(&db).await.unwrap();
        assert!(s.enabled && s.last_hash.is_none() && s.last_error.is_none());
        assert_eq!(s.title.as_deref(), Some("Bar"));
        assert_eq!(
            s.store_id.as_deref(),
            Some("abc"),
            "credenciais não mudam ao salvar a tela"
        );
    }

    #[tokio::test]
    async fn foto_do_produto() {
        let db = Db::open_in_memory().await.unwrap();
        let id = product(&db).await;

        assert!(matches!(
            put_product_image(&db, &id, vec![], "h").await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            put_product_image(&db, &id, b"GIF89a.......".to_vec(), "h").await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            put_product_image(&db, &id, vec![0xFF; MAX_IMAGE_BYTES + 1], "h").await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            put_product_image(&db, "nao-existe", PNG.to_vec(), "h").await,
            Err(DbError::NotFound(_))
        ));

        put_product_image(&db, &id, PNG.to_vec(), "h1")
            .await
            .unwrap();
        mark_image_published(&db, &id, "h1").await.unwrap();
        let img = product_image(&db, &id).await.unwrap().unwrap();
        assert_eq!(
            (img.mime_type.as_str(), img.hash.as_str()),
            ("image/png", "h1")
        );
        assert_eq!(
            images(&db, std::slice::from_ref(&id)).await.unwrap()[0]
                .published_hash
                .as_deref(),
            Some("h1")
        );

        // Foto nova: hash muda, publishedHash fica o antigo até subir
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpeg.extend([1, 2, 3]);
        put_product_image(&db, &id, jpeg, "h2").await.unwrap();
        let r = &images(&db, std::slice::from_ref(&id)).await.unwrap()[0];
        assert_eq!(
            (r.hash.as_str(), r.published_hash.as_deref()),
            ("h2", Some("h1"))
        );

        reset_published_images(&db).await.unwrap();
        assert_eq!(
            images(&db, std::slice::from_ref(&id)).await.unwrap()[0].published_hash,
            None
        );
        delete_product_image(&db, &id).await.unwrap();
        delete_product_image(&db, &id).await.unwrap();
        assert!(product_image(&db, &id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn dados_do_cardapio_filtrados() {
        let db = Db::open_in_memory().await.unwrap();
        let id = product(&db).await;
        let hidden = product(&db).await;
        set_menu_visibility(&db, &[], std::slice::from_ref(&hidden))
            .await
            .unwrap();
        let data = menu_data(&db).await.unwrap();
        assert_eq!(
            data.products
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec![id.as_str()]
        );
        assert_eq!(data.categories.len(), 2);
        assert_eq!(
            title_sources(&db).await.unwrap().0,
            None,
            "sem configuração fiscal"
        );
    }

    #[test]
    fn assinatura_do_arquivo() {
        assert_eq!(detect_image_mime(&PNG), Some("image/png"));
        assert_eq!(
            detect_image_mime(b"RIFF\0\0\0\0WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            detect_image_mime(&[0xFF, 0xD8, 0xFF, 0xDB]),
            Some("image/jpeg")
        );
        assert_eq!(detect_image_mime(b"<svg"), None);
    }
}
