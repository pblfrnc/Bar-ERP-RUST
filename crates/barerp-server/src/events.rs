//! Eventos do Socket.IO com os payloads do Node.

use barerp_core::pricing::kitchen_note;
use barerp_db::events::Event;
use serde_json::{json, Value};

use crate::prisma::{self, str_field};
use crate::AppState;

/// Evento emitido para os aparelhos, repassado também à tela nativa ([`crate::ServerHandle::subscribe`]).
#[derive(Debug, Clone)]
pub struct ServerEvent {
    /// Nome no Socket.IO (`table:updated`, `order:updated`, `cash:updated`, `kds:*`, `devices_updated`...)
    pub name: String,
    pub payload: Value,
}

/// Emite para todos os aparelhos conectados e avisa a tela nativa.
pub fn broadcast(state: &AppState, name: &str, payload: Value) {
    if let Err(e) = state.io.emit(name, &payload) {
        tracing::warn!("falha ao emitir {name}: {e}");
    }
    // Sem ninguém inscrito o envio falha, e tudo bem
    let _ = state.events.send(ServerEvent { name: name.to_string(), payload });
}

pub async fn emit_all(state: &AppState, events: &[Event]) {
    for event in events {
        let payload = match event {
            Event::TableUpdated { table_id, action } => json!({ "tableId": table_id, "action": action }),
            Event::OrderUpdated { order_id, action } => json!({ "orderId": order_id, "action": action }),
            Event::CashUpdated => Value::Null,
            Event::CatalogUpdated { scope } => json!({ "scope": scope }),
            Event::KdsNewOrder { order_id, item_ids } => match kds_new_order(state, order_id, item_ids).await {
                Ok(Some(payload)) => payload,
                Ok(None) => continue,
                Err(e) => {
                    tracing::error!("kds:new_order da comanda {order_id}: {e}");
                    continue;
                }
            },
            Event::KdsItemUpdated { item_id, status, station, table_id, order_id } => json!({
                "itemId": item_id,
                "status": status,
                "station": station,
                "tableId": table_id,
                "orderId": order_id,
            }),
            Event::KdsItemReady { .. } => match serde_json::to_value(event) {
                Ok(mut v) => {
                    if let Value::Object(ref mut map) = v {
                        map.remove("event");
                    }
                    v
                }
                Err(_) => continue,
            },
            Event::KdsBatchUpdated { order_id, station, status } => json!({
                "orderId": order_id,
                "station": station,
                "status": status,
            }),
            Event::KdsOrderReady { .. } => match serde_json::to_value(event) {
                Ok(mut v) => {
                    if let Value::Object(ref mut map) = v {
                        map.remove("event");
                    }
                    v
                }
                Err(_) => continue,
            },
        };
        broadcast(state, event.socket_name(), payload);
    }
}

/// `kds:new_order` como o Node: itens com `product` e a observação da cozinha pronta em `notes`.
/// Na venda de balcão (sem mesa) os itens que não vão para cozinha/bar (`NONE`) ficam de fora;
/// no lançamento em mesa o Node mandava todos.
pub(crate) async fn kds_new_order(state: &AppState, order_id: &str, item_ids: &[String]) -> sqlx::Result<Option<Value>> {
    let mut conn = state.db.pool().acquire().await?;
    let Some(order) = prisma::select_one(&mut conn, "Order", order_id).await? else {
        return Ok(None);
    };
    let mut items = prisma::select_in(&mut conn, "OrderItem", "id", item_ids, r#"ORDER BY "addedAt" ASC, rowid ASC"#).await?;
    if str_field(&order, "tableId").is_none() {
        items.retain(|i| str_field(i, "kdsStation") != Some("NONE"));
    }
    if items.is_empty() {
        return Ok(None);
    }
    prisma::attach_products(&mut conn, &mut items).await?;
    for item in items.iter_mut() {
        let note = kitchen_note(str_field(item, "modifiers"), str_field(item, "notes"));
        item.insert("notes".into(), note.map(Value::String).unwrap_or(Value::Null));
    }

    let table = match str_field(&order, "tableId") {
        Some(tid) => prisma::select_one(&mut conn, "Table", tid).await?,
        None => None,
    };
    let waiter = str_field(&order, "waiterName").filter(|w| !w.is_empty());
    let mut payload = json!({
        "items": items,
        "orderNumber": order.get("orderNumber"),
    });
    match table {
        Some(t) => {
            let name = str_field(&t, "name")
                .filter(|n| !n.is_empty())
                .map(String::from)
                .unwrap_or_else(|| format!("Mesa {}", t.get("number").cloned().unwrap_or(Value::Null)));
            payload["tableName"] = json!(name);
            payload["tableNumber"] = t.get("number").cloned().unwrap_or(Value::Null);
            payload["waiterName"] = json!(waiter.unwrap_or("Garçom"));
        }
        None => {
            payload["tableName"] = json!("Balcão");
            payload["waiterName"] = json!(waiter.unwrap_or("Balcão / Caixa"));
        }
    }
    Ok(Some(payload))
}
