//! Linhas do `bar.db` no JSON que o Prisma devolvia: todas as colunas do modelo, datas em ISO 8601 com
//! milissegundos (`2026-10-18T15:27:10.588Z`), booleanos `true/false` e números como números.
//!
//! O tipo vem da coluna declarada no DDL (`DATETIME`, `BOOLEAN`, `REAL`...), que é o mesmo do Prisma.

use barerp_core::time;
use chrono::SecondsFormat;
use serde_json::{Map, Number, Value};
use sqlx::sqlite::{SqliteArguments, SqliteRow};
use sqlx::{Column, Row, SqliteConnection, TypeInfo, ValueRef};

pub type Obj = Map<String, Value>;

/// Converte uma célula conforme o tipo declarado da coluna.
fn cell(row: &SqliteRow, idx: usize) -> Value {
    let raw = match row.try_get_raw(idx) {
        Ok(raw) if !raw.is_null() => raw,
        _ => return Value::Null,
    };
    let stored = raw.type_info().name().to_string();
    let declared = row.columns()[idx].type_info().name().to_uppercase();

    if declared.contains("DATE") || declared.contains("TIME") {
        let parsed = match stored.as_str() {
            "TEXT" => row.try_get::<String, _>(idx).ok().and_then(|s| time::parse_text(&s)),
            "REAL" => row.try_get::<f64, _>(idx).ok().and_then(|ms| time::from_millis(ms as i64)),
            _ => row.try_get::<i64, _>(idx).ok().and_then(time::from_millis),
        };
        return parsed
            .map(|dt| Value::String(dt.to_rfc3339_opts(SecondsFormat::Millis, true)))
            .unwrap_or(Value::Null);
    }
    if declared.contains("BOOL") {
        return Value::Bool(row.try_get::<i64, _>(idx).map(|v| v != 0).unwrap_or(false));
    }
    match stored.as_str() {
        "INTEGER" => row.try_get::<i64, _>(idx).map(Value::from).unwrap_or(Value::Null),
        "REAL" => row
            .try_get::<f64, _>(idx)
            .ok()
            .map(|v| {
                // Coluna Float do Prisma com valor inteiro sai como 10, não 10.0 (no JSON é igual)
                if v.fract() == 0.0 && v.abs() < 9e15 {
                    Value::from(v as i64)
                } else {
                    Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null)
                }
            })
            .unwrap_or(Value::Null),
        "BLOB" => Value::Null,
        _ => row.try_get::<String, _>(idx).map(Value::String).unwrap_or(Value::Null),
    }
}

pub fn row_to_obj(row: &SqliteRow) -> Obj {
    row.columns().iter().enumerate().map(|(i, col)| (col.name().to_string(), cell(row, i))).collect()
}

/// `SELECT * FROM "<tabela>" <resto>` com parâmetros de texto, já no formato do Prisma.
pub async fn select(conn: &mut SqliteConnection, table: &str, rest: &str, binds: &[&str]) -> sqlx::Result<Vec<Obj>> {
    let sql = format!(r#"SELECT * FROM "{table}" {rest}"#);
    let mut query = sqlx::query::<sqlx::Sqlite>(&sql);
    for b in binds {
        query = query.bind(b.to_string());
    }
    Ok(query.fetch_all(conn).await?.iter().map(row_to_obj).collect())
}

/// Igual a [`select`], com a lista de ids num `IN (...)`.
pub async fn select_in(
    conn: &mut SqliteConnection,
    table: &str,
    column: &str,
    ids: &[String],
    order: &str,
) -> sqlx::Result<Vec<Obj>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!(r#"SELECT * FROM "{table}" WHERE "{column}" IN ({marks}) {order}"#);
    let mut args = SqliteArguments::default();
    for id in ids {
        sqlx::Arguments::add(&mut args, id.clone()).map_err(sqlx::Error::Encode)?;
    }
    Ok(sqlx::query_with(&sql, args).fetch_all(conn).await?.iter().map(row_to_obj).collect())
}

pub async fn select_one(conn: &mut SqliteConnection, table: &str, id: &str) -> sqlx::Result<Option<Obj>> {
    Ok(select(conn, table, r#"WHERE "id" = ?"#, &[id]).await?.into_iter().next())
}

pub fn str_field<'a>(obj: &'a Obj, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

/// Prende `include: { product: true }` em cada item: `product` = linha do produto pelo `productId`.
pub async fn attach_products(conn: &mut SqliteConnection, items: &mut [Obj]) -> sqlx::Result<()> {
    let mut ids: Vec<String> = items.iter().filter_map(|i| str_field(i, "productId").map(String::from)).collect();
    ids.sort();
    ids.dedup();
    let products = select_in(conn, "Product", "id", &ids, "").await?;
    for item in items.iter_mut() {
        let product = str_field(item, "productId")
            .and_then(|pid| products.iter().find(|p| str_field(p, "id") == Some(pid)))
            .cloned()
            .map(Value::Object)
            .unwrap_or(Value::Null);
        item.insert("product".into(), product);
    }
    Ok(())
}

/// Ordenação dos itens/pagamentos da comanda, como nos `include` do Node.
#[derive(Clone, Copy)]
pub enum Sort {
    /// `orderBy: { addedAt/receivedAt: 'desc' }`
    Desc,
    /// `orderBy: { ...: 'asc' }`
    Asc,
    /// Sem `orderBy` (o Prisma devolve na ordem do banco)
    None,
}

impl Sort {
    fn sql(self, column: &str) -> String {
        match self {
            // Itens lançados juntos têm o mesmo milissegundo: desempata pela ordem de gravação
            Sort::Desc => format!(r#"ORDER BY "{column}" DESC, rowid DESC"#),
            Sort::Asc => format!(r#"ORDER BY "{column}" ASC, rowid ASC"#),
            Sort::None => "ORDER BY rowid".into(),
        }
    }
}

/// O que acompanha a comanda na resposta.
#[derive(Clone, Copy)]
pub struct OrderInclude {
    pub table: bool,
    pub items: Option<Sort>,
    pub payments: Option<Sort>,
}

impl OrderInclude {
    /// `include: { table: true, items: { include: { product: true } }, payments: true }`
    pub const FULL: Self = Self { table: true, items: Some(Sort::None), payments: Some(Sort::None) };
}

/// Comanda com `table`, `items` (cada um com `product`) e `payments`, no formato do Prisma.
pub async fn order_with(conn: &mut SqliteConnection, order: Obj, include: OrderInclude) -> sqlx::Result<Obj> {
    let mut list = orders_with(conn, vec![order], include).await?;
    Ok(list.remove(0))
}

pub async fn orders_with(conn: &mut SqliteConnection, mut orders: Vec<Obj>, include: OrderInclude) -> sqlx::Result<Vec<Obj>> {
    for order in orders.iter_mut() {
        let id = str_field(order, "id").unwrap_or_default().to_string();
        if include.table {
            let table = match str_field(order, "tableId") {
                Some(tid) => select_one(conn, "Table", tid).await?.map(Value::Object).unwrap_or(Value::Null),
                None => Value::Null,
            };
            order.insert("table".into(), table);
        }
        if let Some(sort) = include.items {
            let mut items = select(conn, "OrderItem", &format!(r#"WHERE "orderId" = ? {}"#, sort.sql("addedAt")), &[&id]).await?;
            attach_products(conn, &mut items).await?;
            order.insert("items".into(), Value::Array(items.into_iter().map(Value::Object).collect()));
        }
        if let Some(sort) = include.payments {
            let payments = select(conn, "Payment", &format!(r#"WHERE "orderId" = ? {}"#, sort.sql("receivedAt")), &[&id]).await?;
            order.insert("payments".into(), Value::Array(payments.into_iter().map(Value::Object).collect()));
        }
    }
    Ok(orders)
}

/// Busca a comanda pelo id e monta os `include`.
pub async fn find_order(conn: &mut SqliteConnection, id: &str, include: OrderInclude) -> sqlx::Result<Option<Obj>> {
    match select_one(conn, "Order", id).await? {
        Some(order) => Ok(Some(order_with(conn, order, include).await?)),
        None => Ok(None),
    }
}
