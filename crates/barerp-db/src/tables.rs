//! Mesas do salão com o resumo da comanda aberta (mapa de mesas).

use serde::Serialize;
use sqlx::FromRow;

use crate::{Db, DbResult, DbTime};

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TableCard {
    pub id: String,
    pub number: i64,
    pub name: Option<String>,
    pub capacity: i64,
    /// AVAILABLE, OCCUPIED, CLOSING, RESERVED
    pub status: String,
    pub section: String,
    #[sqlx(rename = "customerCount")]
    pub customer_count: i64,
    #[sqlx(rename = "customerName")]
    pub customer_name: Option<String>,
    #[sqlx(rename = "openedAt")]
    pub opened_at: Option<DbTime>,
    #[sqlx(rename = "mergedIntoTableId")]
    pub merged_into_table_id: Option<String>,
    #[sqlx(rename = "orderId")]
    pub order_id: Option<String>,
    #[sqlx(rename = "waiterName")]
    pub waiter_name: Option<String>,
    /// Total da comanda aberta (com taxa de serviço e desconto)
    pub total: Option<f64>,
    /// Já abatido (haver)
    #[sqlx(rename = "paidAmount")]
    pub paid_amount: Option<f64>,
    #[sqlx(rename = "itemCount")]
    pub item_count: i64,
    /// Linhas da comanda (o cartão original mostra "Consumo (N itens)" com o número de linhas)
    #[sqlx(rename = "lineCount")]
    pub line_count: i64,
    /// 10% ligado na comanda aberta
    #[sqlx(rename = "feeActive")]
    pub fee_active: Option<bool>,
}

/// Todas as mesas em ordem de número, com a comanda aberta mais recente de cada uma.
pub async fn list_cards(db: &Db) -> DbResult<Vec<TableCard>> {
    Ok(sqlx::query_as(
        r#"SELECT t."id", t."number", t."name", t."capacity", t."status", t."section",
                  t."customerCount", t."customerName", t."openedAt", t."mergedIntoTableId",
                  o."id" AS "orderId", o."waiterName", o."total", o."paidAmount",
                  COALESCE((SELECT SUM(i."quantity") FROM "OrderItem" i WHERE i."orderId" = o."id"), 0) AS "itemCount",
                  (SELECT COUNT(*) FROM "OrderItem" i WHERE i."orderId" = o."id") AS "lineCount",
                  o."isServiceFeeActive" AS "feeActive"
           FROM "Table" t
           LEFT JOIN "Order" o ON o."id" = (
               SELECT o2."id" FROM "Order" o2
               WHERE o2."tableId" = t."id" AND o2."status" = 'OPEN'
               ORDER BY o2."createdAt" DESC LIMIT 1
           )
           ORDER BY t."number" ASC"#,
    )
    .fetch_all(db.pool())
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use barerp_core::{ids, time};

    #[tokio::test]
    async fn card_shows_open_order() {
        let db = Db::open_in_memory().await.unwrap();
        let now = time::now_db();
        let table_id = ids::cuid();
        sqlx::query(r#"INSERT INTO "Table" ("id","number","status","createdAt","updatedAt") VALUES (?,?,?,?,?)"#)
            .bind(&table_id).bind(4).bind("OCCUPIED").bind(now).bind(now)
            .execute(db.pool()).await.unwrap();
        sqlx::query(r#"INSERT INTO "Table" ("id","number","createdAt","updatedAt") VALUES (?,?,?,?)"#)
            .bind(ids::cuid()).bind(1).bind(now).bind(now)
            .execute(db.pool()).await.unwrap();
        sqlx::query(r#"INSERT INTO "Order" ("id","tableId","status","total","paidAmount","createdAt","updatedAt") VALUES (?,?,?,?,?,?,?)"#)
            .bind(ids::cuid()).bind(&table_id).bind("OPEN").bind(87.5).bind(20.0).bind(now).bind(now)
            .execute(db.pool()).await.unwrap();

        let cards = list_cards(&db).await.unwrap();
        assert_eq!(cards.iter().map(|c| c.number).collect::<Vec<_>>(), vec![1, 4]);
        assert!(cards[0].order_id.is_none());
        assert_eq!((cards[1].total, cards[1].paid_amount), (Some(87.5), Some(20.0)));
        assert_eq!(cards[1].section, "Salão Principal");
    }
}
