//! Leituras da área Caixa & PDV: pagamentos do turno (com a mesa) e o cardápio da venda de balcão.
//! As gravações (abrir/fechar turno, sangria, venda de balcão) ficam em `cash.rs` e `orders.rs`.

use sqlx::FromRow;

use crate::{Db, DbResult, DbTime};

/// Pagamento recebido no turno, para a lista "Movimentações & Pagamentos do Turno".
#[derive(Debug, Clone, FromRow)]
pub struct TurnPayment {
    pub id: String,
    pub amount: f64,
    pub method: String,
    pub notes: Option<String>,
    #[sqlx(rename = "receivedAt")]
    pub received_at: DbTime,
    /// Nome da mesa (ou "Mesa N"); `None` na venda de balcão
    pub place: Option<String>,
    #[sqlx(rename = "orderNumber")]
    pub order_number: Option<i64>,
}

/// Pagamentos do turno, mais recentes primeiro (como o `include` do `/api/cash/current`).
pub async fn turn_payments(db: &Db, shift_id: &str) -> DbResult<Vec<TurnPayment>> {
    Ok(sqlx::query_as(
        r#"SELECT p."id", p."amount", p."method", p."notes", p."receivedAt",
                  COALESCE(NULLIF(t."name", ''), 'Mesa ' || t."number") AS "place", o."orderNumber"
           FROM "Payment" p
           LEFT JOIN "Order" o ON o."id" = p."orderId"
           LEFT JOIN "Table" t ON t."id" = o."tableId"
           WHERE p."cashShiftId" = ?
           ORDER BY p."receivedAt" DESC, p.rowid DESC"#,
    )
    .bind(shift_id)
    .fetch_all(db.pool())
    .await?)
}

/// Produto do PDV de balcão com tudo o que a busca, o cartão e as conferências da NFC-e usam.
#[derive(Debug, Clone, FromRow)]
pub struct PdvProduct {
    pub id: String,
    pub code: Option<String>,
    pub ean: Option<String>,
    #[sqlx(rename = "boxEan")]
    pub box_ean: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub brand: Option<String>,
    pub price: f64,
    #[sqlx(rename = "categoryId")]
    pub category_id: String,
    #[sqlx(rename = "hasBoxPrice")]
    pub has_box_price: bool,
    #[sqlx(rename = "boxPrice")]
    pub box_price: Option<f64>,
    #[sqlx(rename = "boxQuantity")]
    pub box_quantity: Option<i64>,
    pub stock: i64,
    #[sqlx(rename = "trackStock")]
    pub track_stock: bool,
    #[sqlx(rename = "minStock")]
    pub min_stock: i64,
    pub ncm: Option<String>,
    pub cfop: Option<String>,
    pub cest: Option<String>,
    pub unit: Option<String>,
    /// Tem algum grupo de adicionais ativo (tocar no produto abre a escolha)
    #[sqlx(rename = "hasModifiers")]
    pub has_modifiers: bool,
    /// Tem pergunta obrigatória (o leitor de código de barras também abre a escolha)
    #[sqlx(rename = "hasRequiredModifiers")]
    pub has_required_modifiers: bool,
}

impl PdvProduct {
    /// Caixa fechada vendável (tem preço de caixa).
    pub fn sells_box(&self) -> bool {
        self.has_box_price && self.box_price.is_some_and(|p| p > 0.0)
    }

    pub fn box_units(&self) -> i64 {
        self.box_quantity.filter(|q| *q > 0).unwrap_or(24)
    }

    /// Estoque no mínimo ou abaixo (selo vermelho).
    pub fn low_stock(&self) -> bool {
        self.track_stock && self.stock <= self.min_stock
    }

    /// NCM com 8 dígitos (`isValidNcm` do original): sem ele a NFC-e é recusada.
    pub fn valid_ncm(&self) -> bool {
        let digits: String = self.ncm.as_deref().unwrap_or("").chars().filter(char::is_ascii_digit).collect();
        digits.len() == 8
    }
}

/// Produtos ativos, na ordem do `/api/products` (categoria, nome).
pub async fn pdv_products(db: &Db) -> DbResult<Vec<PdvProduct>> {
    Ok(sqlx::query_as(
        r#"SELECT p."id", p."code", p."ean", p."boxEan", p."name", p."description", p."brand", p."price",
                  p."categoryId", p."hasBoxPrice", p."boxPrice", p."boxQuantity",
                  CAST(p."stock" AS INTEGER) AS "stock", p."trackStock", COALESCE(p."minStock", 0) AS "minStock", p."ncm",
                  p."cfop", p."cest", p."unit",
                  EXISTS (SELECT 1 FROM "ProductModifierGroup" l JOIN "ModifierGroup" g ON g."id" = l."groupId"
                          WHERE l."productId" = p."id" AND g."isActive" = 1) AS "hasModifiers",
                  EXISTS (SELECT 1 FROM "ProductModifierGroup" l JOIN "ModifierGroup" g ON g."id" = l."groupId"
                          WHERE l."productId" = p."id" AND g."isActive" = 1 AND g."minSelect" > 0) AS "hasRequiredModifiers"
           FROM "Product" p
           WHERE p."isActive" = 1
           ORDER BY p."categoryId" ASC, p."name" ASC"#,
    )
    .fetch_all(db.pool())
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use barerp_core::{ids, time};

    #[tokio::test]
    async fn catalog_flags_and_turn_payments() {
        let db = Db::open_in_memory().await.unwrap();
        let now = time::now_db();
        let (cat, beer, burger, group, table, order, shift) =
            (ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid());
        let exec = |sql: &'static str| sqlx::query(sql);
        exec(r#"INSERT INTO "Category" ("id","name") VALUES (?, 'Bar')"#).bind(&cat).execute(db.pool()).await.unwrap();
        for (id, name, ncm) in [(&beer, "Cerveja", "22030000"), (&burger, "Burger", "2106.90")] {
            exec(r#"INSERT INTO "Product" ("id","name","price","categoryId","stock","minStock","ncm","createdAt","updatedAt")
                    VALUES (?,?,10,?,5,10,?,?,?)"#)
                .bind(id).bind(name).bind(&cat).bind(ncm).bind(now).bind(now).execute(db.pool()).await.unwrap();
        }
        exec(r#"INSERT INTO "ModifierGroup" ("id","name","minSelect","maxSelect","createdAt","updatedAt") VALUES (?, 'Ponto', 1, 1, ?, ?)"#)
            .bind(&group).bind(now).bind(now).execute(db.pool()).await.unwrap();
        exec(r#"INSERT INTO "ProductModifierGroup" ("productId","groupId") VALUES (?,?)"#)
            .bind(&burger).bind(&group).execute(db.pool()).await.unwrap();

        let products = pdv_products(&db).await.unwrap();
        let burger_row = products.iter().find(|p| p.id == burger).unwrap();
        let beer_row = products.iter().find(|p| p.id == beer).unwrap();
        assert!(burger_row.has_modifiers && burger_row.has_required_modifiers);
        assert!(!beer_row.has_modifiers);
        assert!(beer_row.valid_ncm() && !burger_row.valid_ncm());
        assert!(beer_row.low_stock());
        assert_eq!(beer_row.box_units(), 24);

        exec(r#"INSERT INTO "Table" ("id","number","createdAt","updatedAt") VALUES (?, 7, ?, ?)"#)
            .bind(&table).bind(now).bind(now).execute(db.pool()).await.unwrap();
        exec(r#"INSERT INTO "CashShift" ("id","openedAt","openedBy","initialBalance","status") VALUES (?, ?, 'Ana', 100, 'OPEN')"#)
            .bind(&shift).bind(now).execute(db.pool()).await.unwrap();
        for (oid, tid, n) in [(&order, Some(&table), 101), (&ids::cuid(), None, 102)] {
            exec(r#"INSERT INTO "Order" ("id","orderNumber","tableId","status","createdAt","updatedAt") VALUES (?,?,?,'PAID',?,?)"#)
                .bind(oid).bind(n).bind(tid).bind(now).bind(now).execute(db.pool()).await.unwrap();
            exec(r#"INSERT INTO "Payment" ("id","orderId","amount","method","cashShiftId","receivedAt") VALUES (?,?,20,'PIX',?,?)"#)
                .bind(ids::cuid()).bind(oid).bind(&shift).bind(now).execute(db.pool()).await.unwrap();
        }
        let payments = turn_payments(&db, &shift).await.unwrap();
        assert_eq!(payments.len(), 2);
        assert_eq!(payments[0].place, None, "balcão, gravado por último");
        assert_eq!(payments[1].place.as_deref(), Some("Mesa 7"));
    }
}
