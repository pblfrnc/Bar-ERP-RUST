//! Telas KDS do bar e da cozinha: fila por praça e andamento dos itens (PENDING → PREPARING → READY → DELIVERED).
//! Tradução de `server/src/routes/kds.ts`, com estas diferenças de propósito:
//! - item que não existe responde "não encontrado" (o Node dava erro 500);
//! - mudança em lote (`batch_status`) não mexe nos itens já entregues: no Node, "começar a preparar"
//!   a comanda devolvia para a fila as bebidas que já tinham sido servidas;
//! - `ready_all` com praça `ALL` vale para todas as praças (no Node não marcava nada);
//! - comanda que não existe responde "não encontrada" (o Node respondia sucesso sem mudar nada).

use chrono::Utc;
use serde::Serialize;
use sqlx::{FromRow, SqliteConnection};

use barerp_core::pricing::kitchen_note;

use crate::events::{Event, KdsReadyItem};
use crate::{Db, DbError, DbResult, DbTime};

/// Status que a tela do KDS aceita.
pub const STATUSES: [&str; 4] = ["PENDING", "PREPARING", "READY", "DELIVERED"];

/// Item na fila do KDS, com a comanda e a mesa.
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct KdsItem {
    pub id: String,
    #[sqlx(rename = "orderId")]
    pub order_id: String,
    #[sqlx(rename = "productId")]
    pub product_id: String,
    #[sqlx(rename = "productName")]
    pub product_name: String,
    pub quantity: i64,
    #[sqlx(rename = "unitType")]
    pub unit_type: Option<String>,
    /// Observação para a cozinha já montada: adicionais + observação do garçom (`kitchen_note`)
    pub notes: Option<String>,
    /// JSON `[{ group, name, price }]`, como gravado
    pub modifiers: Option<String>,
    #[sqlx(rename = "kdsStatus")]
    pub kds_status: String,
    #[sqlx(rename = "kdsStation")]
    pub kds_station: String,
    #[sqlx(rename = "addedAt")]
    pub added_at: DbTime,
    #[sqlx(rename = "orderNumber")]
    pub order_number: i64,
    #[sqlx(rename = "tableId")]
    pub table_id: Option<String>,
    #[sqlx(rename = "tableNumber")]
    pub table_number: Option<i64>,
    #[sqlx(rename = "tableName")]
    pub table_name: Option<String>,
    #[sqlx(rename = "waiterId")]
    pub waiter_id: Option<String>,
    #[sqlx(rename = "waiterName")]
    pub waiter_name: Option<String>,
    #[sqlx(rename = "customerName")]
    pub customer_name: Option<String>,
}

impl KdsItem {
    /// Nome da mesa, "Mesa N" ou "Balcão" (como o aviso de pronto do Node).
    pub fn place_label(&self) -> String {
        place_label(
            self.table_name.as_deref(),
            self.table_number,
            self.table_id.is_some(),
        )
    }

    fn with_kitchen_note(mut self) -> Self {
        self.notes = kitchen_note(self.modifiers.as_deref(), self.notes.as_deref());
        self
    }
}

fn place_label(name: Option<&str>, number: Option<i64>, has_table: bool) -> String {
    match (name.filter(|n| !n.is_empty()), number) {
        _ if !has_table => "Balcão".into(),
        (Some(n), _) => n.to_string(),
        (None, Some(n)) => format!("Mesa {n}"),
        (None, None) => "Balcão".into(),
    }
}

const ITEM_SELECT: &str = r#"SELECT i."id", i."orderId", i."productId", p."name" AS "productName", i."quantity",
       i."unitType", i."notes", i."modifiers", i."kdsStatus", i."kdsStation", i."addedAt",
       o."orderNumber", o."tableId", t."number" AS "tableNumber", t."name" AS "tableName",
       o."waiterId", o."waiterName", o."customerName"
FROM "OrderItem" i
JOIN "Product" p ON p."id" = i."productId"
JOIN "Order" o ON o."id" = i."orderId"
LEFT JOIN "Table" t ON t."id" = o."tableId""#;

/// Praça do filtro: `None`/`ALL` = todas; o resto em maiúsculas (`bar` → `BAR`).
fn station_filter(station: Option<&str>) -> Option<String> {
    station
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("ALL"))
        .map(str::to_uppercase)
}

fn check_status(status: &str) -> DbResult<()> {
    if STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(DbError::rule("Status KDS inválido"))
    }
}

fn iso_now() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub use queue as list;
pub use set_item_status as set_status;

/// Fila da praça (`BAR`, `KITCHEN` ou `ALL`): itens pendentes, em preparo e prontos das comandas abertas,
/// do mais antigo para o mais novo.
pub async fn queue(db: &Db, station: Option<&str>) -> DbResult<Vec<KdsItem>> {
    let station = station_filter(station);
    let sql = format!(
        r#"{ITEM_SELECT}
           WHERE i."kdsStatus" IN ('PENDING', 'PREPARING', 'READY') AND o."status" = 'OPEN'
             AND (? IS NULL OR i."kdsStation" = ?)
           ORDER BY i."addedAt" ASC"#
    );
    let items: Vec<KdsItem> = sqlx::query_as(&sql)
        .bind(&station)
        .bind(&station)
        .fetch_all(db.pool())
        .await?;
    Ok(items.into_iter().map(KdsItem::with_kitchen_note).collect())
}

async fn item(conn: &mut SqliteConnection, item_id: &str) -> DbResult<KdsItem> {
    let item: KdsItem = sqlx::query_as(&format!(r#"{ITEM_SELECT} WHERE i."id" = ?"#))
        .bind(item_id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DbError::not_found("Item não encontrado"))?;
    Ok(item.with_kitchen_note())
}

/// Muda o status de um item. Pronto (`READY`) avisa o garçom.
pub async fn set_item_status(
    db: &Db,
    item_id: &str,
    status: &str,
) -> DbResult<(KdsItem, Vec<Event>)> {
    check_status(status)?;
    let mut tx = db.begin_write().await?;
    let changed =
        sqlx::query(r#"UPDATE "OrderItem" SET "kdsStatus" = ?, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(status)
            .bind(DbTime::now())
            .bind(item_id)
            .execute(&mut **tx)
            .await?
            .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Item não encontrado"));
    }
    let updated = item(&mut tx, item_id).await?;
    tx.commit().await?;

    let mut events = vec![Event::KdsItemUpdated {
        item_id: updated.id.clone(),
        status: updated.kds_status.clone(),
        station: updated.kds_station.clone(),
        table_id: updated.table_id.clone(),
        order_id: updated.order_id.clone(),
    }];
    if let Some(table_id) = &updated.table_id {
        events.push(Event::table(table_id, "kds_status_changed"));
    }
    if status == "READY" {
        events.push(Event::KdsItemReady {
            item_id: updated.id.clone(),
            order_id: updated.order_id.clone(),
            order_number: updated.order_number,
            table_name: updated.place_label(),
            table_number: updated.table_number,
            waiter_id: updated.waiter_id.clone(),
            waiter_name: updated
                .waiter_name
                .clone()
                .filter(|w| !w.is_empty())
                .unwrap_or_else(|| "Garçom".into()),
            station: updated.kds_station.clone(),
            product_name: updated.product_name.clone(),
            quantity: updated.quantity,
            notes: updated.notes.clone(),
            ready_at: iso_now(),
        });
    }
    Ok((updated, events))
}

#[derive(FromRow)]
struct OrderInfo {
    #[sqlx(rename = "orderNumber")]
    order_number: i64,
    #[sqlx(rename = "tableId")]
    table_id: Option<String>,
    #[sqlx(rename = "tableNumber")]
    table_number: Option<i64>,
    #[sqlx(rename = "tableName")]
    table_name: Option<String>,
    #[sqlx(rename = "waiterId")]
    waiter_id: Option<String>,
    #[sqlx(rename = "waiterName")]
    waiter_name: Option<String>,
}

/// Muda em lote os itens da comanda que estão em `from` (e na praça, se informada) para `to`.
async fn update_batch(
    db: &Db,
    order_id: &str,
    station: Option<&str>,
    from: &[&str],
    to: &str,
) -> DbResult<Vec<Event>> {
    let filter = station_filter(station);
    let mut tx = db.begin_write().await?;
    let order: OrderInfo = sqlx::query_as(
        r#"SELECT o."orderNumber", o."tableId", t."number" AS "tableNumber", t."name" AS "tableName",
                  o."waiterId", o."waiterName"
           FROM "Order" o LEFT JOIN "Table" t ON t."id" = o."tableId" WHERE o."id" = ?"#,
    )
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| DbError::not_found("Comanda não encontrada"))?;

    let placeholders = vec!["?"; from.len()].join(", ");
    let affected: Vec<KdsItem> = {
        let sql = format!(
            r#"{ITEM_SELECT} WHERE i."orderId" = ? AND i."kdsStatus" IN ({placeholders}) AND (? IS NULL OR i."kdsStation" = ?) ORDER BY i."addedAt" ASC"#
        );
        let mut q = sqlx::query_as::<_, KdsItem>(&sql).bind(order_id);
        for s in from {
            q = q.bind(*s);
        }
        q.bind(&filter).bind(&filter).fetch_all(&mut **tx).await?
    };
    let update_sql = format!(
        r#"UPDATE "OrderItem" SET "kdsStatus" = ?, "updatedAt" = ? WHERE "orderId" = ? AND "kdsStatus" IN ({placeholders}) AND (? IS NULL OR "kdsStation" = ?)"#
    );
    let mut q = sqlx::query(&update_sql)
        .bind(to)
        .bind(DbTime::now())
        .bind(order_id);
    for s in from {
        q = q.bind(*s);
    }
    q.bind(&filter).bind(&filter).execute(&mut **tx).await?;
    tx.commit().await?;

    let mut events = vec![
        Event::KdsBatchUpdated {
            order_id: order_id.to_string(),
            station: station.map(str::to_string),
            status: to.to_string(),
        },
        Event::order(order_id, "kds_status_changed"),
    ];
    if to == "READY" && !affected.is_empty() {
        events.push(Event::KdsOrderReady {
            order_id: order_id.to_string(),
            order_number: order.order_number,
            table_name: place_label(
                order.table_name.as_deref(),
                order.table_number,
                order.table_id.is_some(),
            ),
            table_number: order.table_number,
            waiter_id: order.waiter_id,
            waiter_name: order
                .waiter_name
                .filter(|w| !w.is_empty())
                .unwrap_or_else(|| "Garçom".into()),
            station: station
                .filter(|s| !s.is_empty())
                .unwrap_or("COZINHA")
                .to_string(),
            items: affected
                .into_iter()
                .map(|i| {
                    let i = i.with_kitchen_note();
                    KdsReadyItem {
                        name: i.product_name,
                        quantity: i.quantity,
                        notes: i.notes,
                    }
                })
                .collect(),
            ready_at: iso_now(),
        });
    }
    Ok(events)
}

/// Marca como prontos todos os itens pendentes/em preparo da comanda (na praça, se informada) e avisa o garçom.
pub async fn ready_all(db: &Db, order_id: &str, station: Option<&str>) -> DbResult<Vec<Event>> {
    update_batch(db, order_id, station, &["PENDING", "PREPARING"], "READY").await
}

/// Marca como prontos todos os itens pendentes/em preparo da fila (na praça, se informada).
pub async fn ready_all_queue(db: &Db, station: Option<&str>) -> DbResult<Vec<Event>> {
    let items = queue(db, station).await?;
    let mut order_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for it in &items {
        order_ids.insert(it.order_id.clone());
    }
    let mut all_events = Vec::new();
    for oid in order_ids {
        let events = ready_all(db, &oid, station).await?;
        all_events.extend(events);
    }
    Ok(all_events)
}

/// Muda em lote o status dos itens da comanda (na praça, se informada). Itens já entregues ficam como estão,
/// a não ser para quem já está no status pedido.
pub async fn batch_status(
    db: &Db,
    order_id: &str,
    status: &str,
    station: Option<&str>,
) -> DbResult<Vec<Event>> {
    check_status(status)?;
    let from: Vec<&str> = STATUSES
        .iter()
        .copied()
        .filter(|s| *s != status && *s != "DELIVERED")
        .collect();
    update_batch(db, order_id, station, &from, status).await
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::orders::{self, NewLine, OpenTable};
    use barerp_core::ids;

    struct Fx {
        db: Db,
        order: String,
        table: String,
        beer: String,
        burger: String,
    }

    async fn exec(db: &Db, sql: &str, binds: &[&str]) {
        let mut q = sqlx::query(sql);
        for b in binds {
            q = q.bind(*b);
        }
        q.execute(db.pool()).await.unwrap();
    }

    async fn fx() -> Fx {
        let db = Db::open_in_memory().await.unwrap();
        let now = DbTime::now().0.timestamp_millis().to_string();
        let n = now.as_str();
        let (cat, table, beer, burger) = (ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid());
        exec(
            &db,
            r#"INSERT INTO "Category" ("id","name") VALUES (?, 'Geral')"#,
            &[&cat],
        )
        .await;
        exec(&db, r#"INSERT INTO "Table" ("id","number","name","createdAt","updatedAt") VALUES (?, 7, NULL, ?, ?)"#, &[&table, n, n]).await;
        let product = r#"INSERT INTO "Product" ("id","name","price","categoryId","stock","trackStock","kdsStation","createdAt","updatedAt")
                         VALUES (?,?,?,?,100,0,?,?,?)"#;
        exec(&db, product, &[&beer, "Chopp", "10", &cat, "BAR", n, n]).await;
        exec(
            &db,
            product,
            &[&burger, "X-Burguer", "30", &cat, "KITCHEN", n, n],
        )
        .await;
        let (order, _) = orders::open_table(
            &db,
            &table,
            OpenTable {
                waiter_name: Some("Ana".into()),
                customer_count: 2,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let noon = NaiveDate::from_ymd_opt(2026, 10, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let lines = [
            NewLine {
                product_id: beer.clone(),
                quantity: 2,
                modifier_option_ids: Some(vec![]),
                ..Default::default()
            },
            NewLine {
                product_id: burger.clone(),
                quantity: 1,
                notes: Some(" sem cebola ".into()),
                modifier_option_ids: Some(vec![]),
                ..Default::default()
            },
        ];
        orders::add_items(&db, &order, &lines, noon).await.unwrap();
        Fx {
            db,
            order,
            table,
            beer,
            burger,
        }
    }

    async fn status_of(db: &Db, product: &str) -> String {
        sqlx::query_scalar(r#"SELECT "kdsStatus" FROM "OrderItem" WHERE "productId" = ?"#)
            .bind(product)
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn fila_por_praca_com_observacao() {
        let f = fx().await;
        let all = queue(&f.db, Some("ALL")).await.unwrap();
        assert_eq!(all.len(), 2);
        let kitchen = queue(&f.db, Some("kitchen")).await.unwrap();
        assert_eq!(kitchen.len(), 1);
        assert_eq!(kitchen[0].product_name, "X-Burguer");
        assert_eq!(kitchen[0].notes.as_deref(), Some("sem cebola"));
        assert_eq!(kitchen[0].place_label(), "Mesa 7");
        assert_eq!(queue(&f.db, Some("BAR")).await.unwrap()[0].quantity, 2);
    }

    #[tokio::test]
    async fn item_pronto_avisa_o_garcom_e_entregue_sai_da_fila() {
        let f = fx().await;
        let id = queue(&f.db, Some("KITCHEN")).await.unwrap()[0].id.clone();
        assert!(matches!(
            set_item_status(&f.db, &id, "PRONTO").await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            set_item_status(&f.db, "nao-existe", "READY").await,
            Err(DbError::NotFound(_))
        ));

        let (item, events) = set_item_status(&f.db, &id, "READY").await.unwrap();
        assert_eq!(item.kds_status, "READY");
        assert_eq!(events.len(), 3);
        assert!(
            matches!(&events[1], Event::TableUpdated { table_id, action: "kds_status_changed" } if *table_id == f.table)
        );
        match &events[2] {
            Event::KdsItemReady {
                table_name,
                waiter_name,
                product_name,
                notes,
                station,
                ..
            } => {
                assert_eq!(
                    (table_name.as_str(), waiter_name.as_str()),
                    ("Mesa 7", "Ana")
                );
                assert_eq!(
                    (product_name.as_str(), station.as_str()),
                    ("X-Burguer", "KITCHEN")
                );
                assert_eq!(notes.as_deref(), Some("sem cebola"));
            }
            e => panic!("evento inesperado: {e:?}"),
        }

        let (_, events) = set_item_status(&f.db, &id, "DELIVERED").await.unwrap();
        assert_eq!(events.len(), 2, "entregue não avisa de novo");
        assert!(queue(&f.db, Some("KITCHEN")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn lote_nao_devolve_entregues_para_a_fila() {
        let f = fx().await;
        let beer_id = queue(&f.db, Some("BAR")).await.unwrap()[0].id.clone();
        set_item_status(&f.db, &beer_id, "DELIVERED").await.unwrap();

        let events = batch_status(&f.db, &f.order, "PREPARING", None)
            .await
            .unwrap();
        assert_eq!(status_of(&f.db, &f.burger).await, "PREPARING");
        assert_eq!(status_of(&f.db, &f.beer).await, "DELIVERED");
        assert!(
            matches!(&events[0], Event::KdsBatchUpdated { station: None, status, .. } if status == "PREPARING")
        );
        assert_eq!(events.len(), 2, "só READY avisa o garçom");

        assert!(matches!(
            batch_status(&f.db, &f.order, "X", None).await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            batch_status(&f.db, "nao-existe", "READY", None).await,
            Err(DbError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn pronto_em_lote_por_praca() {
        let f = fx().await;
        let events = ready_all(&f.db, &f.order, Some("KITCHEN")).await.unwrap();
        assert_eq!(status_of(&f.db, &f.burger).await, "READY");
        assert_eq!(status_of(&f.db, &f.beer).await, "PENDING");
        match events.last().unwrap() {
            Event::KdsOrderReady {
                items,
                station,
                table_name,
                ..
            } => {
                assert_eq!(station, "KITCHEN");
                assert_eq!(table_name, "Mesa 7");
                assert_eq!(
                    items,
                    &vec![KdsReadyItem {
                        name: "X-Burguer".into(),
                        quantity: 1,
                        notes: Some("sem cebola".into())
                    }]
                );
            }
            e => panic!("evento inesperado: {e:?}"),
        }
        // Nada mais pendente na cozinha: não avisa de novo
        let events = ready_all(&f.db, &f.order, Some("KITCHEN")).await.unwrap();
        assert_eq!(events.len(), 2);
        // ALL vale para todas as praças
        ready_all(&f.db, &f.order, Some("ALL")).await.unwrap();
        assert_eq!(status_of(&f.db, &f.beer).await, "READY");
    }
}
