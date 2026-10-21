//! Comandas finalizadas (histórico, 2ª via) e baixa de comanda como perda (calote).
//! Tradução de `GET /orders/history` e `POST /orders/:id/loss` de `server/src/routes/orders.ts`, com estas
//! diferenças de propósito:
//! - o filtro por data usa o dia LOCAL do PC (o Node usava o dia em UTC: vendas depois das 21h caíam no dia seguinte);
//! - a perda usa o total gravado da comanda (o Node recalculava com 10% fixo, ignorando a taxa configurada).

use chrono::{Local, NaiveDate, TimeZone, Utc};

use barerp_core::money::brl;

use crate::events::Event;
use crate::orders::OrderHead;
use crate::{audit, Db, DbError, DbResult, DbTime};

#[derive(Debug, Clone, Default)]
pub struct HistoryFilter {
    /// Dia (local) do fechamento; `None` = todos
    pub date: Option<NaiveDate>,
    /// Comanda #, mesa, cliente ou garçom
    pub search: String,
    pub limit: i64,
}

/// Início e fim do dia local em milissegundos UTC (formato do banco).
fn local_day_bounds(day: NaiveDate) -> (i64, i64) {
    let start = Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("meia-noite existe"))
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&day.and_hms_opt(3, 0, 0).expect("hora válida")));
    (start.timestamp_millis(), start.timestamp_millis() + 24 * 3600 * 1000 - 1)
}

/// Comandas pagas/fechadas, mais recentes primeiro.
pub async fn finished_orders(db: &Db, filter: &HistoryFilter) -> DbResult<Vec<OrderHead>> {
    let limit = if filter.limit > 0 { filter.limit.min(500) } else { 100 };
    let (from, to) = filter.date.map(local_day_bounds).unwrap_or((i64::MIN, i64::MAX));
    let search = filter.search.trim().to_lowercase();
    let like = format!("%{search}%");
    let rows: Vec<OrderHead> = sqlx::query_as(
        r#"SELECT o."id", o."orderNumber", o."tableId", o."status", o."customerName", o."waiterName",
                  o."subtotal", o."serviceFeeRate", o."serviceFee", o."discount", o."total", o."paidAmount",
                  o."isServiceFeeActive", o."createdAt", o."closedAt", t."number" AS "tableNumber", t."name" AS "tableName"
           FROM "Order" o LEFT JOIN "Table" t ON t."id" = o."tableId"
           WHERE o."status" IN ('PAID', 'CLOSED')
             AND o."closedAt" IS NOT NULL
             -- closedAt pode estar em ms (Prisma) ou texto antigo: compara pelo valor em ms
             AND (CASE WHEN typeof(o."closedAt") = 'integer' THEN o."closedAt"
                       ELSE CAST(strftime('%s', o."closedAt") AS INTEGER) * 1000 END) BETWEEN ? AND ?
             AND (? = '' OR CAST(o."orderNumber" AS TEXT) LIKE ? OR CAST(t."number" AS TEXT) = ?
                  OR LOWER(COALESCE(t."name", '')) LIKE ? OR LOWER(COALESCE(o."customerName", '')) LIKE ?
                  OR LOWER(COALESCE(o."waiterName", '')) LIKE ?)
           ORDER BY o."closedAt" DESC
           LIMIT ?"#,
    )
    .bind(from)
    .bind(to)
    .bind(&search)
    .bind(&like)
    .bind(&search)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

/// Baixa a comanda como perda (calote/evasão): fecha sem pagamento, libera a mesa e registra na auditoria.
/// Devolve o valor perdido.
pub async fn mark_as_loss(db: &Db, order_id: &str, user: Option<&str>) -> DbResult<(f64, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    // número, status, mesa, total, pago, número da mesa
    type LossRow = (i64, String, Option<String>, f64, f64, Option<i64>);
    let row: Option<LossRow> = sqlx::query_as(
        r#"SELECT o."orderNumber", o."status", o."tableId", o."total", o."paidAmount", t."number"
           FROM "Order" o LEFT JOIN "Table" t ON t."id" = o."tableId" WHERE o."id" = ?"#,
    )
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?;
    let (number, status, table_id, total, paid, table_number) =
        row.ok_or_else(|| DbError::not_found("Comanda não encontrada"))?;
    if status != "OPEN" {
        return Err(DbError::rule("Comanda já fechada."));
    }
    let lost = (total - paid).max(0.0);
    let now = DbTime::now();
    sqlx::query(r#"UPDATE "Order" SET "status" = 'CLOSED', "closedAt" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(now)
        .bind(now)
        .bind(order_id)
        .execute(&mut **tx)
        .await?;
    let mut events = vec![Event::order(order_id, "closed_loss")];
    if let Some(table_id) = &table_id {
        sqlx::query(
            r#"UPDATE "Table" SET "status" = 'AVAILABLE', "currentOrderId" = NULL, "customerName" = NULL,
                                  "customerCount" = 1, "openedAt" = NULL, "updatedAt" = ?
               WHERE "id" = ?"#,
        )
        .bind(now)
        .bind(table_id)
        .execute(&mut **tx)
        .await?;
        events.push(Event::table(table_id, "table_freed"));
    }
    if lost > 0.0 {
        let place = table_number.map_or("Balcão".to_string(), |n| n.to_string());
        audit::record(
            &mut tx,
            "LOSS_CALOTE",
            &format!("Perda (Calote/Evasão) de {} na Comanda #{number} (Mesa {place})", brl(lost)),
            user,
        )
        .await?;
    }
    tx.commit().await?;
    Ok((lost, events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{add_items, open_table, OpenTable};
    use crate::orders::tests::{fixture, line, noon};

    #[tokio::test]
    async fn loss_and_history() {
        let f = fixture().await;
        let (order, _) = open_table(&f.db, &f.table1, OpenTable { customer_name: Some("Mariana".into()), ..Default::default() })
            .await
            .unwrap();
        add_items(&f.db, &order, &[line(&f.beer, 3)], noon()).await.unwrap();
        let (lost, events) = mark_as_loss(&f.db, &order, Some("Gerente")).await.unwrap();
        assert_eq!(lost, 24.0);
        assert!(events.contains(&Event::table(&f.table1, "table_freed")));
        assert!(mark_as_loss(&f.db, &order, None).await.is_err());

        let all = finished_orders(&f.db, &HistoryFilter::default()).await.unwrap();
        assert_eq!(all.len(), 1);
        let by_name = finished_orders(&f.db, &HistoryFilter { search: "mari".into(), ..Default::default() }).await.unwrap();
        assert_eq!(by_name.len(), 1);
        let today = Local::now().date_naive();
        let today_rows = finished_orders(&f.db, &HistoryFilter { date: Some(today), ..Default::default() }).await.unwrap();
        assert_eq!(today_rows.len(), 1);
        let yesterday = today.pred_opt().unwrap();
        assert!(finished_orders(&f.db, &HistoryFilter { date: Some(yesterday), ..Default::default() })
            .await
            .unwrap()
            .is_empty());

        let audit: String = sqlx::query_scalar(r#"SELECT "description" FROM "AuditLog" WHERE "action" = 'LOSS_CALOTE'"#)
            .fetch_one(f.db.pool())
            .await
            .unwrap();
        assert_eq!(audit, "Perda (Calote/Evasão) de R$ 24,00 na Comanda #101 (Mesa 1)");
    }
}
