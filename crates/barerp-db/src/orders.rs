//! Comandas: abrir mesa, lançar e cancelar itens, taxa de serviço, desconto, transferir e juntar mesas.
//! Tradução de `server/src/routes/orders.ts` e `tables.ts`, com estas diferenças de propósito:
//! - tudo numa transação (queda de energia no meio não deixa comanda pela metade);
//! - cancelar item vendido como caixa fechada devolve `quantidade × unidades da caixa` ao estoque
//!   (o Node devolvia só `quantidade`, embora a baixa usasse o multiplicador);
//! - produto que não existe mais recusa o lançamento com mensagem (o Node pulava o item calado);
//! - subtotal, taxa e total gravados arredondados em centavos.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection};

use barerp_core::money::brl;
use barerp_core::ids;
use barerp_core::pricing::{self, round2, ProductForPricing};

use crate::events::Event;
use crate::{audit, catalog, Db, DbError, DbResult, DbTime};

/// Diferença tolerada nas conferências de haver (mesma do Node)
const HAVER_TOLERANCE: f64 = 0.05;
const DEFAULT_BOX_QUANTITY: i64 = 24;
const HAVER_ERROR: &str =
    "O cliente já abateu (haver) mais do que o novo total da conta ficaria. Ajuste os pagamentos antes.";

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OrderHead {
    pub id: String,
    #[sqlx(rename = "orderNumber")]
    pub order_number: i64,
    #[sqlx(rename = "tableId")]
    pub table_id: Option<String>,
    pub status: String,
    #[sqlx(rename = "customerName")]
    pub customer_name: Option<String>,
    #[sqlx(rename = "waiterName")]
    pub waiter_name: Option<String>,
    pub subtotal: f64,
    #[sqlx(rename = "serviceFeeRate")]
    pub service_fee_rate: f64,
    #[sqlx(rename = "serviceFee")]
    pub service_fee: f64,
    pub discount: f64,
    pub total: f64,
    #[sqlx(rename = "paidAmount")]
    pub paid_amount: f64,
    #[sqlx(rename = "isServiceFeeActive")]
    pub is_service_fee_active: bool,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
    #[sqlx(rename = "closedAt")]
    pub closed_at: Option<DbTime>,
    #[sqlx(rename = "tableNumber")]
    pub table_number: Option<i64>,
    #[sqlx(rename = "tableName")]
    pub table_name: Option<String>,
}

impl OrderHead {
    /// Quanto falta pagar.
    pub fn remaining(&self) -> f64 {
        (self.total - self.paid_amount).max(0.0)
    }

    /// "Mesa 4", o nome da mesa ou "Balcão".
    pub fn place_label(&self) -> String {
        match (&self.table_name, self.table_number) {
            (Some(name), _) if !name.is_empty() => name.clone(),
            (_, Some(n)) => format!("Mesa {n}"),
            _ => "Balcão".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OrderLine {
    pub id: String,
    #[sqlx(rename = "productId")]
    pub product_id: String,
    #[sqlx(rename = "productName")]
    pub product_name: String,
    pub quantity: i64,
    #[sqlx(rename = "unitPrice")]
    pub unit_price: f64,
    #[sqlx(rename = "totalPrice")]
    pub total_price: f64,
    #[sqlx(rename = "unitType")]
    pub unit_type: Option<String>,
    pub notes: Option<String>,
    /// JSON `[{ group, name, price }]`
    pub modifiers: Option<String>,
    #[sqlx(rename = "basePrice")]
    pub base_price: Option<f64>,
    #[sqlx(rename = "promotionName")]
    pub promotion_name: Option<String>,
    #[sqlx(rename = "kdsStatus")]
    pub kds_status: String,
    #[sqlx(rename = "kdsStation")]
    pub kds_station: String,
    #[sqlx(rename = "paidQuantity")]
    pub paid_quantity: i64,
    #[sqlx(rename = "addedAt")]
    pub added_at: DbTime,
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PaymentLine {
    pub id: String,
    pub amount: f64,
    pub method: String,
    pub notes: Option<String>,
    #[sqlx(rename = "receivedAt")]
    pub received_at: DbTime,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderDetail {
    pub head: OrderHead,
    /// Mais recentes primeiro (como a tela do Node)
    pub items: Vec<OrderLine>,
    pub payments: Vec<PaymentLine>,
}

const HEAD_SELECT: &str = r#"SELECT o."id", o."orderNumber", o."tableId", o."status", o."customerName", o."waiterName",
       o."subtotal", o."serviceFeeRate", o."serviceFee", o."discount", o."total", o."paidAmount",
       o."isServiceFeeActive", o."createdAt", o."closedAt", t."number" AS "tableNumber", t."name" AS "tableName"
FROM "Order" o LEFT JOIN "Table" t ON t."id" = o."tableId""#;

async fn head(conn: &mut SqliteConnection, order_id: &str) -> DbResult<OrderHead> {
    sqlx::query_as(&format!(r#"{HEAD_SELECT} WHERE o."id" = ?"#))
        .bind(order_id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DbError::not_found("Comanda não encontrada"))
}

async fn open_head(conn: &mut SqliteConnection, order_id: &str) -> DbResult<OrderHead> {
    let order = head(conn, order_id).await?;
    if order.status != "OPEN" {
        return Err(DbError::Conflict("Esta comanda já foi fechada ou cancelada. Atualize a tela.".into()));
    }
    Ok(order)
}

pub async fn detail(db: &Db, order_id: &str) -> DbResult<OrderDetail> {
    let mut conn = db.pool().acquire().await?;
    let head = head(&mut conn, order_id).await?;
    let items = sqlx::query_as(
        r#"SELECT i."id", i."productId", p."name" AS "productName", i."quantity", i."unitPrice", i."totalPrice",
                  i."unitType", i."notes", i."modifiers", i."basePrice", i."promotionName", i."kdsStatus",
                  i."kdsStation", i."paidQuantity", i."addedAt"
           FROM "OrderItem" i JOIN "Product" p ON p."id" = i."productId"
           WHERE i."orderId" = ? ORDER BY i."addedAt" DESC"#,
    )
    .bind(order_id)
    .fetch_all(&mut *conn)
    .await?;
    let payments = sqlx::query_as(
        r#"SELECT "id", "amount", "method", "notes", "receivedAt" FROM "Payment"
           WHERE "orderId" = ? ORDER BY "receivedAt" DESC"#,
    )
    .bind(order_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(OrderDetail { head, items, payments })
}

/// Comanda aberta mais recente da mesa.
pub async fn open_order_of_table(conn: &mut SqliteConnection, table_id: &str) -> DbResult<Option<String>> {
    Ok(sqlx::query_scalar(
        r#"SELECT "id" FROM "Order" WHERE "tableId" = ? AND "status" = 'OPEN' ORDER BY "createdAt" DESC LIMIT 1"#,
    )
    .bind(table_id)
    .fetch_optional(conn)
    .await?)
}

pub(crate) async fn next_order_number(conn: &mut SqliteConnection) -> DbResult<i64> {
    let last: Option<i64> = sqlx::query_scalar(r#"SELECT MAX("orderNumber") FROM "Order""#).fetch_one(conn).await?;
    Ok(last.unwrap_or(100) + 1)
}

/// Recalcula subtotal, taxa e total a partir dos itens. Devolve o novo total.
pub(crate) async fn recalc_totals(conn: &mut SqliteConnection, order_id: &str) -> DbResult<f64> {
    let (subtotal,): (f64,) =
        sqlx::query_as(r#"SELECT COALESCE(SUM("totalPrice"), 0.0) FROM "OrderItem" WHERE "orderId" = ?"#)
            .bind(order_id)
            .fetch_one(&mut *conn)
            .await?;
    let (active, rate, discount): (bool, f64, f64) =
        sqlx::query_as(r#"SELECT "isServiceFeeActive", "serviceFeeRate", "discount" FROM "Order" WHERE "id" = ?"#)
            .bind(order_id)
            .fetch_one(&mut *conn)
            .await?;
    let (subtotal, fee, total) = totals(subtotal, active, rate, discount);
    sqlx::query(r#"UPDATE "Order" SET "subtotal" = ?, "serviceFee" = ?, "total" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(subtotal)
        .bind(fee)
        .bind(total)
        .bind(DbTime::now())
        .bind(order_id)
        .execute(conn)
        .await?;
    Ok(total)
}

/// (subtotal, taxa, total) em centavos.
fn totals(subtotal: f64, fee_active: bool, rate: f64, discount: f64) -> (f64, f64, f64) {
    let subtotal = round2(subtotal);
    let fee = if fee_active { round2(subtotal * rate) } else { 0.0 };
    (subtotal, fee, round2(subtotal + fee - discount))
}

/// Baixa (`sign = -1`) ou devolução (`sign = 1`) de estoque de `units` unidades avulsas do produto,
/// pela ficha técnica quando houver.
pub(crate) async fn move_stock(conn: &mut SqliteConnection, product_id: &str, units: f64, sign: f64) -> DbResult<()> {
    let components: Vec<(String, f64)> =
        sqlx::query_as(r#"SELECT "componentId", "quantity" FROM "ProductComponent" WHERE "parentProductId" = ?"#)
            .bind(product_id)
            .fetch_all(&mut *conn)
            .await?;
    let now = DbTime::now();
    if components.is_empty() {
        let track_stock: bool = sqlx::query_scalar(r#"SELECT "trackStock" FROM "Product" WHERE "id" = ?"#)
            .bind(product_id)
            .fetch_one(&mut *conn)
            .await?;
        if track_stock {
            sqlx::query(r#"UPDATE "Product" SET "stock" = "stock" + ?, "updatedAt" = ? WHERE "id" = ?"#)
                .bind(sign * units)
                .bind(now)
                .bind(product_id)
                .execute(&mut *conn)
                .await?;
        }
        return Ok(());
    }
    for (component_id, quantity) in components {
        sqlx::query(r#"UPDATE "Product" SET "stock" = "stock" + ?, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(sign * quantity * units)
            .bind(now)
            .bind(&component_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

async fn free_table(conn: &mut SqliteConnection, table_id: &str) -> DbResult<()> {
    sqlx::query(
        r#"UPDATE "Table" SET "status" = 'AVAILABLE', "currentOrderId" = NULL, "customerName" = NULL,
                              "customerCount" = 1, "openedAt" = NULL, "updatedAt" = ?
           WHERE "id" = ?"#,
    )
    .bind(DbTime::now())
    .bind(table_id)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct OpenTable {
    pub customer_name: Option<String>,
    pub waiter_id: Option<String>,
    pub waiter_name: Option<String>,
    pub customer_count: i64,
}

/// Abre a mesa com uma comanda nova (10% desligado). Devolve o id da comanda.
pub async fn open_table(db: &Db, table_id: &str, input: OpenTable) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let status: String = sqlx::query_scalar(r#"SELECT "status" FROM "Table" WHERE "id" = ?"#)
        .bind(table_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::not_found("Mesa não encontrada"))?;
    // O Node só recusava OCCUPIED; mesa em fechamento também já tem comanda aberta
    if status == "OCCUPIED" || status == "CLOSING" || open_order_of_table(&mut tx, table_id).await?.is_some() {
        return Err(DbError::Conflict("Mesa já está ocupada".into()));
    }

    let customer_name = input.customer_name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let order_id = ids::cuid();
    let now = DbTime::now();
    let number = next_order_number(&mut tx).await?;
    sqlx::query(
        r#"INSERT INTO "Order" ("id", "orderNumber", "tableId", "status", "customerName", "waiterId", "waiterName",
                                "subtotal", "serviceFeeRate", "serviceFee", "discount", "total", "paidAmount",
                                "isServiceFeeActive", "createdAt", "updatedAt")
           VALUES (?, ?, ?, 'OPEN', ?, ?, ?, 0, 0.10, 0, 0, 0, 0, 0, ?, ?)"#,
    )
    .bind(&order_id)
    .bind(number)
    .bind(table_id)
    .bind(&customer_name)
    .bind(&input.waiter_id)
    .bind(input.waiter_name.as_deref().filter(|w| !w.is_empty()).unwrap_or("Garçom"))
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"UPDATE "Table" SET "status" = 'OCCUPIED', "currentOrderId" = ?, "customerName" = ?, "customerCount" = ?,
                              "openedAt" = ?, "updatedAt" = ?
           WHERE "id" = ?"#,
    )
    .bind(&order_id)
    .bind(&customer_name)
    .bind(input.customer_count.max(1))
    .bind(now)
    .bind(now)
    .bind(table_id)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok((order_id, vec![Event::table(table_id, "opened")]))
}

/// Linha pedida pelo garçom/caixa.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewLine {
    pub product_id: String,
    pub quantity: i64,
    pub notes: Option<String>,
    /// "BOX" para caixa fechada
    pub unit_type: Option<String>,
    /// `None` = app antigo, que não sabe escolher adicionais
    pub modifier_option_ids: Option<Vec<String>>,
}

#[derive(FromRow)]
struct ProductRow {
    id: String,
    name: String,
    #[sqlx(rename = "categoryId")]
    category_id: String,
    price: f64,
    #[sqlx(rename = "boxPrice")]
    box_price: Option<f64>,
    #[sqlx(rename = "hasBoxPrice")]
    has_box_price: bool,
    #[sqlx(rename = "boxQuantity")]
    box_quantity: Option<i64>,
    #[sqlx(rename = "kdsStation")]
    kds_station: String,
}

pub(crate) struct PricedNewLine {
    pub product_id: String,
    pub kds_station: String,
    pub quantity: i64,
    pub priced: pricing::PricedLine,
    pub notes: Option<String>,
    /// Unidades avulsas que saem do estoque por unidade vendida (caixa = unidades da caixa)
    pub units_per_item: i64,
}

/// Precifica e valida todas as linhas antes de gravar qualquer uma.
pub(crate) async fn price_lines(
    conn: &mut SqliteConnection,
    lines: &[NewLine],
    now_local: NaiveDateTime,
) -> DbResult<Vec<PricedNewLine>> {
    let promotions = catalog::active_promotions(conn).await?;
    let mut priced = Vec::with_capacity(lines.len());
    for line in lines {
        let product: ProductRow = sqlx::query_as(
            r#"SELECT "id", "name", "categoryId", "price", "boxPrice", "hasBoxPrice", "boxQuantity", "kdsStation"
               FROM "Product" WHERE "id" = ?"#,
        )
        .bind(&line.product_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| DbError::rule("Um dos produtos não existe mais. Atualize o cardápio e lance de novo."))?;

        let groups: Vec<_> = catalog::modifier_groups(conn, &product.id).await?.into_iter().map(|(_, g)| g).collect();
        let pricing_input = ProductForPricing {
            id: &product.id,
            name: &product.name,
            category_id: &product.category_id,
            price: product.price,
            box_price: product.box_price,
            has_box_price: product.has_box_price,
        };
        let result = pricing::price_order_line(
            &pricing_input,
            line.unit_type.as_deref(),
            &groups,
            line.modifier_option_ids.as_deref(),
            &promotions,
            now_local,
        )
        .map_err(|e| DbError::Rule(e.0))?;

        let box_quantity = product.box_quantity.unwrap_or(DEFAULT_BOX_QUANTITY);
        let notes = line
            .notes
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .or_else(|| result.is_box.then(|| format!("📦 Caixa Fechada ({box_quantity} un)")));
        priced.push(PricedNewLine {
            units_per_item: if result.is_box { box_quantity } else { 1 },
            product_id: product.id,
            kds_station: product.kds_station,
            quantity: line.quantity.max(1),
            priced: result,
            notes,
        });
    }
    Ok(priced)
}

/// Grava as linhas já precificadas, dá baixa no estoque e devolve os ids criados.
pub(crate) async fn insert_lines(
    conn: &mut SqliteConnection,
    order_id: &str,
    lines: &[PricedNewLine],
    kds_status_for: impl Fn(&str) -> &'static str,
    paid: bool,
) -> DbResult<Vec<String>> {
    let now = DbTime::now();
    let mut ids_created = Vec::with_capacity(lines.len());
    for line in lines {
        let id = ids::cuid();
        sqlx::query(
            r#"INSERT INTO "OrderItem" ("id", "orderId", "productId", "quantity", "unitPrice", "totalPrice", "unitType",
                                        "notes", "paidQuantity", "modifiers", "basePrice", "promotionName",
                                        "kdsStatus", "kdsStation", "addedAt", "updatedAt")
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(order_id)
        .bind(&line.product_id)
        .bind(line.quantity)
        .bind(line.priced.unit_price)
        .bind(round2(line.priced.unit_price * line.quantity as f64))
        .bind(if line.priced.is_box { "BOX" } else { "UNIT" })
        .bind(&line.notes)
        .bind(if paid { line.quantity } else { 0 })
        .bind(&line.priced.modifiers)
        .bind(line.priced.base_price)
        .bind(&line.priced.promotion_name)
        .bind(kds_status_for(&line.kds_station))
        .bind(&line.kds_station)
        .bind(now)
        .bind(now)
        .execute(&mut *conn)
        .await?;
        move_stock(conn, &line.product_id, (line.quantity * line.units_per_item) as f64, -1.0).await?;
        ids_created.push(id);
    }
    Ok(ids_created)
}

/// Lança itens na comanda aberta. `now_local` é o relógio do PC (vale para as promoções do dia).
pub async fn add_items(
    db: &Db,
    order_id: &str,
    lines: &[NewLine],
    now_local: NaiveDateTime,
) -> DbResult<(Vec<String>, Vec<Event>)> {
    if lines.is_empty() {
        return Err(DbError::rule("Nenhum item informado"));
    }
    let mut tx = db.begin_write().await?;
    let order = open_head(&mut tx, order_id).await?;
    let priced = price_lines(&mut tx, lines, now_local).await?;
    let created = insert_lines(&mut tx, order_id, &priced, |_| "PENDING", false).await?;
    recalc_totals(&mut tx, order_id).await?;

    let mut events = vec![Event::order(order_id, "items_added")];
    if let Some(table_id) = &order.table_id {
        // Mesa em fechamento volta a ocupada: chegou pedido novo
        sqlx::query(r#"UPDATE "Table" SET "status" = 'OCCUPIED', "updatedAt" = ? WHERE "id" = ?"#)
            .bind(DbTime::now())
            .bind(table_id)
            .execute(&mut **tx)
            .await?;
        events.push(Event::table(table_id, "items_added"));
    }
    tx.commit().await?;
    events.push(Event::KdsNewOrder { order_id: order_id.into(), item_ids: created.clone() });
    Ok((created, events))
}

#[derive(FromRow)]
struct RemovedItemRow {
    #[sqlx(rename = "productId")]
    product_id: String,
    name: String,
    quantity: i64,
    #[sqlx(rename = "totalPrice")]
    total_price: f64,
    #[sqlx(rename = "unitType")]
    unit_type: Option<String>,
    #[sqlx(rename = "boxQuantity")]
    box_quantity: Option<i64>,
}

/// Cancela um item da comanda, devolve o estoque e registra na auditoria.
pub async fn remove_item(db: &Db, order_id: &str, item_id: &str, user: Option<&str>) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let order = open_head(&mut tx, order_id).await?;
    let item: Option<RemovedItemRow> = sqlx::query_as(
        r#"SELECT i."productId", p."name", i."quantity", i."totalPrice", i."unitType", p."boxQuantity"
           FROM "OrderItem" i JOIN "Product" p ON p."id" = i."productId"
           WHERE i."id" = ? AND i."orderId" = ?"#,
    )
    .bind(item_id)
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await?;
    let item = item.ok_or_else(|| DbError::not_found("Item não encontrado no pedido"))?;

    if order.paid_amount > 0.0 {
        let (_, _, new_total) = totals(
            order.subtotal - item.total_price,
            order.is_service_fee_active,
            order.service_fee_rate,
            order.discount,
        );
        if new_total < order.paid_amount - HAVER_TOLERANCE {
            return Err(DbError::rule(HAVER_ERROR));
        }
    }

    let units_per_item = if item.unit_type.as_deref() == Some("BOX") { item.box_quantity.unwrap_or(DEFAULT_BOX_QUANTITY) } else { 1 };
    move_stock(&mut tx, &item.product_id, (item.quantity * units_per_item) as f64, 1.0).await?;
    sqlx::query(r#"DELETE FROM "OrderItem" WHERE "id" = ?"#).bind(item_id).execute(&mut **tx).await?;
    audit::record(
        &mut tx,
        "CANCEL_ITEM",
        &format!("Cancelado {}x {} (Comanda #{} - {})", item.quantity, item.name, order.order_number, order.place_label()),
        user,
    )
    .await?;
    recalc_totals(&mut tx, order_id).await?;
    tx.commit().await?;

    let mut events = vec![Event::order(order_id, "item_removed")];
    events.extend(order.table_id.as_deref().map(|t| Event::table(t, "item_removed")));
    Ok(events)
}

/// Liga/desliga os 10% (ou muda a taxa).
pub async fn set_service_fee(db: &Db, order_id: &str, active: Option<bool>, rate: Option<f64>) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let order = open_head(&mut tx, order_id).await?;
    let active = active.unwrap_or(!order.is_service_fee_active);
    let rate = rate.unwrap_or(order.service_fee_rate);
    let (_, fee, total) = totals(order.subtotal, active, rate, order.discount);
    if total < order.paid_amount - HAVER_TOLERANCE {
        return Err(DbError::rule(HAVER_ERROR));
    }
    sqlx::query(
        r#"UPDATE "Order" SET "isServiceFeeActive" = ?, "serviceFeeRate" = ?, "serviceFee" = ?, "total" = ?, "updatedAt" = ?
           WHERE "id" = ?"#,
    )
    .bind(active)
    .bind(rate)
    .bind(fee)
    .bind(total)
    .bind(DbTime::now())
    .bind(order_id)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    let mut events = vec![Event::order(order_id, "service_fee")];
    events.extend(order.table_id.as_deref().map(|t| Event::table(t, "service_fee")));
    Ok(events)
}

/// Desconto em reais na comanda (registra na auditoria).
pub async fn set_discount(db: &Db, order_id: &str, discount: f64, user: Option<&str>) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let order = open_head(&mut tx, order_id).await?;
    let discount = round2(discount.max(0.0));
    let total = round2((order.subtotal + order.service_fee - discount).max(0.0));
    if total < order.paid_amount - HAVER_TOLERANCE {
        return Err(DbError::rule(HAVER_ERROR));
    }
    sqlx::query(r#"UPDATE "Order" SET "discount" = ?, "total" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(discount)
        .bind(total)
        .bind(DbTime::now())
        .bind(order_id)
        .execute(&mut **tx)
        .await?;
    if discount > 0.0 {
        audit::record(
            &mut tx,
            "APPLY_DISCOUNT",
            &format!("Desconto de {} na Comanda #{}", brl(discount), order.order_number),
            user,
        )
        .await?;
    }
    tx.commit().await?;
    let mut events = vec![Event::order(order_id, "discount")];
    events.extend(order.table_id.as_deref().map(|t| Event::table(t, "discount")));
    Ok(events)
}

async fn set_table_status(db: &Db, table_id: &str, status: &str, action: &'static str) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let changed = sqlx::query(r#"UPDATE "Table" SET "status" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(status)
        .bind(DbTime::now())
        .bind(table_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Mesa não encontrada"));
    }
    tx.commit().await?;
    Ok(vec![Event::table(table_id, action)])
}

/// Conta pedida / pré-conta impressa.
pub async fn request_closing(db: &Db, table_id: &str) -> DbResult<Vec<Event>> {
    set_table_status(db, table_id, "CLOSING", "closing").await
}

/// Volta de "em fechamento" para ocupada.
pub async fn reopen(db: &Db, table_id: &str) -> DbResult<Vec<Event>> {
    set_table_status(db, table_id, "OCCUPIED", "reopened").await
}

#[derive(FromRow)]
struct TableState {
    number: i64,
    status: String,
    #[sqlx(rename = "customerName")]
    customer_name: Option<String>,
    #[sqlx(rename = "customerCount")]
    customer_count: i64,
    #[sqlx(rename = "openedAt")]
    opened_at: Option<DbTime>,
}

async fn table_state(conn: &mut SqliteConnection, id: &str) -> DbResult<Option<TableState>> {
    Ok(sqlx::query_as(
        r#"SELECT "number", "status", "customerName", "customerCount", "openedAt" FROM "Table" WHERE "id" = ?"#,
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Transfere a comanda inteira para uma mesa livre. Devolve a mensagem de sucesso.
pub async fn transfer_table(db: &Db, source_id: &str, target_id: &str) -> DbResult<(String, Vec<Event>)> {
    if source_id == target_id {
        return Err(DbError::rule("Mesa de origem e destino devem ser diferentes"));
    }
    let mut tx = db.begin_write().await?;
    let (Some(source), Some(target)) = (table_state(&mut tx, source_id).await?, table_state(&mut tx, target_id).await?)
    else {
        return Err(DbError::not_found("Mesa de origem ou destino não encontrada"));
    };
    let order_id = open_order_of_table(&mut tx, source_id)
        .await?
        .ok_or_else(|| DbError::rule("Mesa de origem não possui pedido ativo"))?;
    if target.status != "AVAILABLE" || open_order_of_table(&mut tx, target_id).await?.is_some() {
        return Err(DbError::rule("Mesa de destino precisa estar livre para transferência total"));
    }

    let now = DbTime::now();
    sqlx::query(r#"UPDATE "Order" SET "tableId" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(target_id)
        .bind(now)
        .bind(&order_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        r#"UPDATE "Table" SET "status" = ?, "currentOrderId" = ?, "customerName" = ?, "customerCount" = ?,
                              "openedAt" = ?, "updatedAt" = ?
           WHERE "id" = ?"#,
    )
    .bind(&source.status)
    .bind(&order_id)
    .bind(&source.customer_name)
    .bind(source.customer_count)
    .bind(source.opened_at)
    .bind(now)
    .bind(target_id)
    .execute(&mut **tx)
    .await?;
    free_table(&mut tx, source_id).await?;
    tx.commit().await?;

    Ok((
        format!("Mesa {} transferida para Mesa {}", source.number, target.number),
        vec![Event::table(source_id, "transferred_from"), Event::table(target_id, "transferred_to")],
    ))
}

/// Junta a segunda mesa na principal: itens e pagamentos (haver) vão junto; a segunda fica livre.
pub async fn merge_tables(db: &Db, main_id: &str, second_id: &str) -> DbResult<(String, Vec<Event>)> {
    if main_id == second_id {
        return Err(DbError::rule("Escolha outra mesa para juntar"));
    }
    let mut tx = db.begin_write().await?;
    let (Some(main), Some(second)) = (table_state(&mut tx, main_id).await?, table_state(&mut tx, second_id).await?)
    else {
        return Err(DbError::not_found("Mesas não encontradas"));
    };
    let main_order = open_order_of_table(&mut tx, main_id)
        .await?
        .ok_or_else(|| DbError::rule("Mesa principal não possui comanda ativa"))?;
    let now = DbTime::now();

    if let Some(second_order) = open_order_of_table(&mut tx, second_id).await? {
        for table in ["OrderItem", "Payment"] {
            sqlx::query(&format!(r#"UPDATE "{table}" SET "orderId" = ? WHERE "orderId" = ?"#))
                .bind(&main_order)
                .bind(&second_order)
                .execute(&mut **tx)
                .await?;
        }
        let (second_paid,): (f64,) = sqlx::query_as(r#"SELECT "paidAmount" FROM "Order" WHERE "id" = ?"#)
            .bind(&second_order)
            .fetch_one(&mut **tx)
            .await?;
        sqlx::query(r#"UPDATE "Order" SET "status" = 'CANCELLED', "paidAmount" = 0, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(now)
            .bind(&second_order)
            .execute(&mut **tx)
            .await?;
        sqlx::query(r#"UPDATE "Order" SET "paidAmount" = "paidAmount" + ?, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(second_paid)
            .bind(now)
            .bind(&main_order)
            .execute(&mut **tx)
            .await?;
        recalc_totals(&mut tx, &main_order).await?;
    }

    free_table(&mut tx, second_id).await?;
    sqlx::query(r#"UPDATE "Table" SET "mergedIntoTableId" = ? WHERE "id" = ?"#)
        .bind(main_id)
        .bind(second_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(r#"UPDATE "Table" SET "customerCount" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(main.customer_count.max(1) + second.customer_count.max(1))
        .bind(now)
        .bind(main_id)
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;

    Ok((
        format!("Mesa {} unida à Mesa {}", second.number, main.number),
        vec![Event::table(main_id, "merged"), Event::table(second_id, "merged")],
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPayment {
    pub amount: f64,
    pub method: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaidItemLine {
    pub item_id: String,
    pub quantity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessPaymentInput {
    pub payments: Vec<NewPayment>,
    pub close_order: Option<bool>,
    pub customer_id: Option<String>,
    pub paid_items: Option<Vec<PaidItemLine>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentResult {
    pub order_id: String,
    pub paid_amount: f64,
    pub is_fully_paid: bool,
    pub status: String,
}

/// Realiza pagamentos na comanda (inclusive abatimento parcial / haver, fiado e racha de itens).
pub async fn process_payment(
    db: &Db,
    order_id: &str,
    input: ProcessPaymentInput,
) -> DbResult<(PaymentResult, Vec<Event>)> {
    if input.payments.is_empty() {
        return Err(DbError::rule("Nenhum pagamento fornecido"));
    }

    let mut tx = db.begin_write().await?;
    let order = open_head(&mut tx, order_id).await?;

    if order.status != "OPEN" {
        return Err(DbError::Conflict(
            "Esta comanda já foi fechada ou unida a outra mesa. Atualize a tela.".into(),
        ));
    }

    // Buscar turno de caixa aberto
    let active_shift_id: Option<String> = sqlx::query_scalar(
        r#"SELECT "id" FROM "CashShift" WHERE "status" = 'OPEN' ORDER BY "openedAt" DESC LIMIT 1"#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    let Some(cash_shift_id) = active_shift_id else {
        return Err(DbError::Forbidden(
            "O Caixa (PDV) está fechado! Abra o turno do caixa antes de fechar pedidos ou receber pagamentos."
                .into(),
        ));
    };

    // Fiado sem cliente: a dívida não ficaria registrada em lugar nenhum (o Node aceitava)
    if input.payments.iter().any(|p| p.method == "CREDIT_TAB" && p.amount > 0.0)
        && input.customer_id.as_deref().is_none_or(|c| c.trim().is_empty())
    {
        return Err(DbError::rule("Escolha o cliente para lançar no fiado."));
    }

    let remaining = (order.total - order.paid_amount).max(0.0);
    let requested_total: f64 = input.payments.iter().map(|p| p.amount.max(0.0)).sum();

    if requested_total > remaining + HAVER_TOLERANCE {
        return Err(DbError::rule(format!(
            "Valor informado (R$ {:.2}) é maior que o saldo restante da conta (R$ {:.2}).",
            requested_total, remaining
        )));
    }

    let now = DbTime::now();
    let mut added_paid_amount = 0.0;

    for p in &input.payments {
        let amt = round2(p.amount);
        if amt <= 0.0 {
            continue;
        }

        let payment_id = ids::cuid();
        sqlx::query(
            r#"INSERT INTO "Payment" ("id", "orderId", "amount", "method", "cashShiftId", "notes", "receivedAt")
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&payment_id)
        .bind(order_id)
        .bind(amt)
        .bind(&p.method)
        .bind(&cash_shift_id)
        .bind(&p.notes)
        .bind(now)
        .execute(&mut **tx)
        .await?;

        // Atualizar saldo do fiado se for CREDIT_TAB
        if p.method == "CREDIT_TAB" {
            if let Some(ref cust_id) = input.customer_id {
                sqlx::query(r#"UPDATE "Customer" SET "creditTabBalance" = "creditTabBalance" + ?, "updatedAt" = ? WHERE "id" = ?"#)
                    .bind(amt)
                    .bind(now)
                    .bind(cust_id)
                    .execute(&mut **tx)
                    .await?;
            }
        }

        added_paid_amount += amt;
    }

    // Atualizar paidQuantity dos itens (Racha por Itens)
    if let Some(ref paid_items) = input.paid_items {
        for pi in paid_items {
            if pi.quantity > 0 {
                // Só itens desta comanda, e nunca mais unidades pagas do que o item tem
                let changed = sqlx::query(
                    r#"UPDATE "OrderItem" SET "paidQuantity" = MIN("quantity", "paidQuantity" + ?), "updatedAt" = ?
                       WHERE "id" = ? AND "orderId" = ?"#,
                )
                .bind(pi.quantity)
                .bind(now)
                .bind(&pi.item_id)
                .bind(order_id)
                .execute(&mut **tx)
                .await?
                .rows_affected();
                if changed == 0 {
                    return Err(DbError::Conflict("Um dos itens pagos não está mais nesta comanda. Atualize a tela.".into()));
                }
            }
        }
    }

    let new_paid_total = round2(order.paid_amount + added_paid_amount);
    let should_close = input
        .close_order
        .unwrap_or(new_paid_total >= order.total - HAVER_TOLERANCE);

    if let Some(ref cust_id) = input.customer_id {
        sqlx::query(r#"UPDATE "Order" SET "customerId" = ?, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(cust_id)
            .bind(now)
            .bind(order_id)
            .execute(&mut **tx)
            .await?;
    }

    let final_status = if should_close { "PAID" } else { "OPEN" };
    let closed_at = if should_close { Some(now) } else { None };

    sqlx::query(
        r#"UPDATE "Order"
           SET "paidAmount" = ?, "status" = ?, "closedAt" = ?, "updatedAt" = ?
           WHERE "id" = ?"#,
    )
    .bind(new_paid_total)
    .bind(final_status)
    .bind(closed_at)
    .bind(now)
    .bind(order_id)
    .execute(&mut **tx)
    .await?;

    let mut events = vec![Event::order(order_id, "paid"), Event::CashUpdated];

    // Se fechou e tem mesa, libera a mesa
    if should_close {
        if let Some(ref table_id) = order.table_id {
            free_table(&mut tx, table_id).await?;
            events.push(Event::table(table_id, "closed"));
        }
    }

    tx.commit().await?;

    Ok((
        PaymentResult {
            order_id: order_id.to_string(),
            paid_amount: new_paid_total,
            is_fully_paid: should_close,
            status: final_status.to_string(),
        },
        events,
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickSalePayment {
    pub method: String,
    pub amount: Option<f64>,
    pub cash_tendered: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickSaleInput {
    pub items: Vec<NewLine>,
    pub payment: QuickSalePayment,
    pub customer_name: Option<String>,
    pub discount: Option<f64>,
    pub expected_total: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickSaleResult {
    pub order: OrderDetail,
    pub change: f64,
}

/// Realiza venda rápida de balcão (sem mesa), criando o pedido já com status PAID e dando baixa no estoque e no caixa.
pub async fn quick_sale(
    db: &Db,
    input: QuickSaleInput,
    now_local: NaiveDateTime,
) -> DbResult<(QuickSaleResult, Vec<Event>)> {
    if input.items.is_empty() {
        return Err(DbError::rule("Nenhum item informado para a venda rápida"));
    }

    let mut tx = db.begin_write().await?;

    // 1. Verificar turno de caixa aberto
    let active_shift_id: Option<String> = sqlx::query_scalar(
        r#"SELECT "id" FROM "CashShift" WHERE "status" = 'OPEN' ORDER BY "openedAt" DESC LIMIT 1"#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    let Some(cash_shift_id) = active_shift_id else {
        return Err(DbError::Forbidden(
            "O Caixa (PDV) está fechado! Abra o turno do caixa antes de realizar vendas.".into(),
        ));
    };

    // 2. Número da comanda
    let order_number = next_order_number(&mut tx).await?;
    let order_id = ids::cuid();

    // 3. Precificar itens com promoções e adicionais
    let priced = price_lines(&mut tx, &input.items, now_local).await?;
    let subtotal: f64 = round2(priced.iter().map(|p| p.priced.unit_price * p.quantity as f64).sum());
    let discount = round2(input.discount.unwrap_or(0.0).min(subtotal).max(0.0));
    let final_total = round2((subtotal - discount).max(0.0));

    // Se a tela calculou o total com os preços anteriores e a promoção mudou, avisar.
    // O valor do pagamento (`payment.amount`, que é o que o Node conferia) vale como total esperado.
    if let Some(expected) = input.expected_total.or(input.payment.amount) {
        if (expected - final_total).abs() > 0.05 {
            return Err(DbError::Conflict(format!(
                "Os preços mudaram (promoção do dia começou ou terminou). Novo total: R$ {:.2}. Confira e conclua de novo.",
                final_total
            )));
        }
    }

    // 4. Criar pedido já fechado (PAID) e sem mesa
    let now = DbTime::now();
    let customer_name = input.customer_name.as_deref().unwrap_or("Cliente Balcão");
    sqlx::query(
        r#"INSERT INTO "Order" ("id", "orderNumber", "tableId", "status", "customerName", "waiterName",
                                "subtotal", "serviceFeeRate", "serviceFee", "discount", "total", "paidAmount",
                                "isServiceFeeActive", "createdAt", "updatedAt", "closedAt")
           VALUES (?, ?, NULL, 'PAID', ?, 'Balcão / Caixa', ?, 0.0, 0.0, ?, ?, ?, 0, ?, ?, ?)"#,
    )
    .bind(&order_id)
    .bind(order_number)
    .bind(customer_name)
    .bind(subtotal)
    .bind(discount)
    .bind(final_total)
    .bind(final_total)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    // 5. Inserir itens com paid = true e baixar estoque
    let item_ids = insert_lines(
        &mut tx,
        &order_id,
        &priced,
        |station| if station == "NONE" { "DELIVERED" } else { "PENDING" },
        true,
    )
    .await?;

    // 6. Criar pagamento associado ao turno de caixa
    // O caixa recebe sempre o total calculado aqui, nunca o valor que a tela mandou
    let payment_amount = final_total;
    let cash_tendered = input.payment.cash_tendered.unwrap_or(payment_amount);
    let change = if input.payment.method == "CASH" && cash_tendered > payment_amount {
        round2(cash_tendered - payment_amount)
    } else {
        0.0
    };
    let payment_notes = if change > 0.0 {
        Some(format!("Troco: R$ {:.2} (Recebido: R$ {:.2})", change, cash_tendered))
    } else {
        input.payment.notes.clone()
    };

    let payment_id = ids::cuid();
    sqlx::query(
        r#"INSERT INTO "Payment" ("id", "orderId", "amount", "method", "notes", "cashShiftId", "receivedAt")
           VALUES (?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&payment_id)
    .bind(&order_id)
    .bind(payment_amount)
    .bind(&input.payment.method)
    .bind(payment_notes)
    .bind(&cash_shift_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    tx.commit().await?;

    let order = detail(db, &order_id).await?;

    let mut events = vec![
        Event::order(&order_id, "quick_sale"),
        Event::CashUpdated,
    ];
    let kds_items: Vec<_> = priced
        .iter()
        .zip(&item_ids)
        .filter(|(p, _)| p.kds_station != "NONE")
        .map(|(_, id)| id.clone())
        .collect();
    if !kds_items.is_empty() {
        events.push(Event::KdsNewOrder {
            order_id: order_id.clone(),
            item_ids: kds_items,
        });
    }

    Ok((QuickSaleResult { order, change }, events))
}

#[cfg(test)]
#[path = "orders_tests.rs"]
pub(crate) mod tests;
