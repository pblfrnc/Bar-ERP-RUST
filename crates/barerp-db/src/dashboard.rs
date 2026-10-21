//! Dashboard da Retaguarda e auditoria cega. Tradução de `server/src/routes/dashboard.ts` e `auditLogs.ts`,
//! com estas diferenças de propósito:
//! - datas gravadas em ms (Prisma) ou em texto (`CURRENT_TIMESTAMP` do SQL antigo) entram no mesmo dia: o Node
//!   comparava só o formato do Prisma e perdia as linhas em texto;
//! - a auditoria ordena pelo instante real (no SQLite, texto vem antes de número num `ORDER BY` simples).

use std::collections::HashMap;

use chrono::{Local, NaiveDate, TimeZone, Timelike, Utc};
use serde::Serialize;
use sqlx::FromRow;

use crate::{Db, DbResult, DbTime};

/// Valor em ms de uma coluna de data em qualquer formato do `bar.db`.
fn ms_expr(col: &str) -> String {
    format!(
        "(CASE typeof({col}) WHEN 'integer' THEN {col} WHEN 'real' THEN CAST({col} AS INTEGER) \
         ELSE CAST(strftime('%s', {col}) AS INTEGER) * 1000 END)"
    )
}

/// Início e fim (exclusivo) do dia local em ms UTC.
pub fn local_day_bounds(day: NaiveDate) -> (i64, i64) {
    let start = Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("meia-noite existe"))
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&day.and_hms_opt(3, 0, 0).expect("hora válida")));
    let next = day.succ_opt().unwrap_or(day);
    let end = Local
        .from_local_datetime(&next.and_hms_opt(0, 0, 0).expect("meia-noite existe"))
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(start + chrono::Duration::days(1));
    (start.timestamp_millis(), end.timestamp_millis())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopProduct {
    pub name: String,
    pub quantity: i64,
    pub total: f64,
    pub station: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HourSales {
    /// "18h"
    pub hour: String,
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaiterPerformance {
    pub name: String,
    pub total: f64,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LowMarginProduct {
    pub id: String,
    pub name: String,
    /// Margem em % arredondada
    pub margin: i64,
    pub price: f64,
    pub cost: f64,
}

/// Resposta do GET `/api/dashboard?date=AAAA-MM-DD`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub today_revenue: f64,
    pub orders_completed_today: i64,
    pub occupied_tables: i64,
    pub total_tables: i64,
    pub occupancy_rate: i64,
    pub average_ticket: f64,
    pub top_products: Vec<TopProduct>,
    /// (forma, valor) na ordem em que apareceram no dia, como o objeto do Node
    pub payment_methods_breakdown: Vec<(String, f64)>,
    pub sales_by_hour: Vec<HourSales>,
    pub waiter_performance: Vec<WaiterPerformance>,
    pub low_margin_products: Vec<LowMarginProduct>,
}

#[derive(FromRow)]
struct PaymentRow {
    amount: f64,
    method: String,
    #[sqlx(rename = "receivedAt")]
    received_at: DbTime,
}

/// Números do dia (local) escolhido.
pub async fn dashboard(db: &Db, day: NaiveDate) -> DbResult<DashboardData> {
    let (from, to) = local_day_bounds(day);

    // Pagamentos recebidos no dia
    let payments: Vec<PaymentRow> = sqlx::query_as(&format!(
        r#"SELECT "amount", "method", "receivedAt" FROM "Payment"
           WHERE {r} >= ? AND {r} < ? ORDER BY {r}"#,
        r = ms_expr(r#""receivedAt""#)
    ))
    .bind(from)
    .bind(to)
    .fetch_all(db.pool())
    .await?;
    let today_revenue: f64 = payments.iter().map(|p| p.amount).sum();

    // Comandas fechadas no dia
    let closed: Vec<(Option<String>, f64)> = sqlx::query_as(&format!(
        r#"SELECT "waiterName", "total" FROM "Order"
           WHERE "status" = 'PAID' AND "closedAt" IS NOT NULL AND {c} >= ? AND {c} < ?"#,
        c = ms_expr(r#""closedAt""#)
    ))
    .bind(from)
    .bind(to)
    .fetch_all(db.pool())
    .await?;
    let orders_completed_today = closed.len() as i64;
    let average_ticket = if closed.is_empty() { 0.0 } else { today_revenue / closed.len() as f64 };

    // Mesas agora
    let (total_tables, occupied_tables): (i64, i64) = sqlx::query_as(
        r#"SELECT COUNT(*), COALESCE(SUM(CASE WHEN "status" IN ('OCCUPIED', 'CLOSING') THEN 1 ELSE 0 END), 0) FROM "Table""#,
    )
    .fetch_one(db.pool())
    .await?;

    // Top 5 produtos lançados no dia
    let items: Vec<(String, String, String, i64, f64)> = sqlx::query_as(&format!(
        r#"SELECT i."productId", p."name", p."kdsStation", i."quantity", i."totalPrice"
           FROM "OrderItem" i JOIN "Product" p ON p."id" = i."productId"
           WHERE {a} >= ? AND {a} < ?
           ORDER BY {a}"#,
        a = ms_expr(r#"i."addedAt""#)
    ))
    .bind(from)
    .bind(to)
    .fetch_all(db.pool())
    .await?;
    let mut order: Vec<String> = Vec::new();
    let mut by_product: HashMap<String, TopProduct> = HashMap::new();
    for (id, name, station, qty, total) in items {
        let entry = by_product.entry(id.clone()).or_insert_with(|| {
            order.push(id.clone());
            TopProduct { name, quantity: 0, total: 0.0, station }
        });
        entry.quantity += qty;
        entry.total += total;
    }
    let mut top_products: Vec<TopProduct> = order.into_iter().filter_map(|id| by_product.remove(&id)).collect();
    // sort estável, como o Array.sort do Node: empates mantêm a ordem de lançamento
    top_products.sort_by_key(|p| std::cmp::Reverse(p.quantity));
    top_products.truncate(5);

    // Formas de pagamento
    let mut payment_methods_breakdown: Vec<(String, f64)> = Vec::new();
    for p in &payments {
        match payment_methods_breakdown.iter_mut().find(|(m, _)| *m == p.method) {
            Some((_, v)) => *v += p.amount,
            None => payment_methods_breakdown.push((p.method.clone(), p.amount)),
        }
    }

    // Vendas por hora (só o intervalo com vendas, uma hora antes e uma depois; sem vendas: 17h a 23h)
    let mut hours = [0.0f64; 24];
    for p in &payments {
        hours[p.received_at.0.with_timezone(&Local).hour() as usize] += p.amount;
    }
    let active: Vec<usize> = (0..24).filter(|h| hours[*h] > 0.0).collect();
    let (min_h, max_h) = match (active.first(), active.last()) {
        (Some(first), Some(last)) => (first.saturating_sub(1), (*last + 1).min(23)),
        _ => (17, 23),
    };
    let sales_by_hour = (min_h..=max_h).map(|h| HourSales { hour: format!("{h}h"), total: hours[h] }).collect();

    // Garçons
    let mut waiter_performance: Vec<WaiterPerformance> = Vec::new();
    for (name, total) in closed {
        let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| "Sem Garçom".into());
        match waiter_performance.iter_mut().find(|w| w.name == name) {
            Some(w) => {
                w.total += total;
                w.count += 1;
            }
            None => waiter_performance.push(WaiterPerformance { name, total, count: 1 }),
        }
    }
    waiter_performance.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));

    // Margem abaixo de 30% (produtos ativos sem ficha técnica)
    let products: Vec<(String, String, f64, Option<f64>)> = sqlx::query_as(
        r#"SELECT p."id", p."name", p."price", p."costPrice" FROM "Product" p
           WHERE p."isActive" = 1
             AND NOT EXISTS (SELECT 1 FROM "ProductComponent" c WHERE c."parentProductId" = p."id")"#,
    )
    .fetch_all(db.pool())
    .await?;
    let mut low_margin_products: Vec<LowMarginProduct> = products
        .into_iter()
        .filter_map(|(id, name, price, cost)| {
            let cost = cost.filter(|c| *c != 0.0)?;
            if price <= 0.0 || (price - cost) / price >= 0.3 {
                return None;
            }
            Some(LowMarginProduct { id, name, margin: (((price - cost) / price) * 100.0).round() as i64, price, cost })
        })
        .collect();
    low_margin_products.sort_by_key(|p| p.margin);

    Ok(DashboardData {
        today_revenue,
        orders_completed_today,
        occupied_tables,
        total_tables,
        occupancy_rate: if total_tables > 0 {
            ((occupied_tables as f64 / total_tables as f64) * 100.0).round() as i64
        } else {
            0
        },
        average_ticket,
        top_products,
        payment_methods_breakdown,
        sales_by_hour,
        waiter_performance,
        low_margin_products,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: String,
    pub action: String,
    pub description: String,
    #[sqlx(rename = "userId")]
    pub user_id: Option<String>,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
}

/// Últimas 100 ações sensíveis, da mais nova para a mais antiga.
pub async fn audit_logs(db: &Db) -> DbResult<Vec<AuditEntry>> {
    Ok(sqlx::query_as(&format!(
        r#"SELECT "id", "action", "description", "userId", "createdAt" FROM "AuditLog"
           ORDER BY {c} DESC LIMIT 100"#,
        c = ms_expr(r#""createdAt""#)
    ))
    .fetch_all(db.pool())
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn exec(db: &Db, sql: &str) {
        sqlx::query(sql).execute(db.pool()).await.unwrap();
    }

    fn at(day: NaiveDate, h: u32, m: u32) -> i64 {
        Local.from_local_datetime(&day.and_hms_opt(h, m, 0).unwrap()).earliest().unwrap().timestamp_millis()
    }

    #[tokio::test]
    async fn dashboard_of_the_day() {
        let db = Db::open_in_memory().await.unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let other = NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
        exec(&db, r#"INSERT INTO "Category" ("id","name") VALUES ('c','Bebidas')"#).await;
        exec(&db, r#"INSERT INTO "Product" ("id","name","price","costPrice","categoryId","kdsStation","isActive","updatedAt")
                     VALUES ('p1','Chopp',10,8,'c','BAR',1,0), ('p2','Fritas',30,9,'c','KITCHEN',1,0), ('p3','Velho',5,4.9,'c','BAR',0,0)"#)
            .await;
        exec(&db, r#"INSERT INTO "Table" ("id","number","status","updatedAt") VALUES ('t1',1,'OCCUPIED',0),('t2',2,'AVAILABLE',0),('t3',3,'CLOSING',0)"#).await;
        exec(
            &db,
            &format!(
                r#"INSERT INTO "Order" ("id","orderNumber","status","waiterName","total","closedAt","updatedAt")
                   VALUES ('o1',1,'PAID','Ana',40,{},0), ('o2',2,'PAID','Bia',60,{},0), ('o3',3,'PAID','Ana',10,{},0), ('o4',4,'OPEN',NULL,0,NULL,0)"#,
                at(day, 20, 0),
                at(day, 22, 30),
                at(other, 23, 0)
            ),
        )
        .await;
        exec(
            &db,
            &format!(
                r#"INSERT INTO "Payment" ("id","orderId","amount","method","receivedAt") VALUES
                   ('pa','o1',40,'PIX',{}), ('pb','o2',50,'CASH',{}), ('pc','o2',10,'PIX',{}), ('pd','o3',10,'PIX',{})"#,
                at(day, 20, 0),
                at(day, 22, 30),
                at(day, 22, 40),
                at(other, 23, 0)
            ),
        )
        .await;
        exec(
            &db,
            &format!(
                r#"INSERT INTO "OrderItem" ("id","orderId","productId","quantity","unitPrice","totalPrice","addedAt","updatedAt") VALUES
                   ('i1','o1','p1',2,10,20,{a},0), ('i2','o2','p2',1,30,30,{a},0), ('i3','o2','p1',3,10,30,{a},0), ('i4','o3','p2',9,30,270,{b},0)"#,
                a = at(day, 19, 0),
                b = at(other, 19, 0)
            ),
        )
        .await;

        let d = dashboard(&db, day).await.unwrap();
        assert_eq!(d.today_revenue, 100.0);
        assert_eq!(d.orders_completed_today, 2);
        assert_eq!(d.average_ticket, 50.0);
        assert_eq!((d.occupied_tables, d.total_tables, d.occupancy_rate), (2, 3, 67));
        assert_eq!(d.top_products[0], TopProduct { name: "Chopp".into(), quantity: 5, total: 50.0, station: "BAR".into() });
        assert_eq!(d.top_products[1].name, "Fritas");
        assert_eq!(d.payment_methods_breakdown, vec![("PIX".into(), 50.0), ("CASH".into(), 50.0)]);
        assert_eq!(d.sales_by_hour.first().unwrap().hour, "19h");
        assert_eq!(d.sales_by_hour.last().unwrap().hour, "23h");
        assert_eq!(d.sales_by_hour.iter().find(|h| h.hour == "22h").unwrap().total, 60.0);
        assert_eq!(d.waiter_performance[0], WaiterPerformance { name: "Bia".into(), total: 60.0, count: 1 });
        assert_eq!(d.low_margin_products.len(), 1);
        assert_eq!(d.low_margin_products[0].name, "Chopp");
        assert_eq!(d.low_margin_products[0].margin, 20);

        let empty = dashboard(&db, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).await.unwrap();
        assert_eq!(empty.sales_by_hour.first().unwrap().hour, "17h");
        assert!(empty.top_products.is_empty());
    }

    #[tokio::test]
    async fn audit_sorted_by_real_time() {
        let db = Db::open_in_memory().await.unwrap();
        exec(&db, r#"INSERT INTO "AuditLog" ("id","action","description","createdAt") VALUES ('a','CANCEL_ITEM','velho','2026-01-01 10:00:00')"#).await;
        exec(&db, &format!(r#"INSERT INTO "AuditLog" ("id","action","description","createdAt") VALUES ('b','APPLY_DISCOUNT','novo',{})"#, Utc::now().timestamp_millis())).await;
        let logs = audit_logs(&db).await.unwrap();
        assert_eq!(logs.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
    }
}
