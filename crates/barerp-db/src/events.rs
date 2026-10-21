//! O que mudou depois de uma gravação. A tela nativa usa para se atualizar e o servidor repassa aos
//! celulares com os mesmos nomes de evento do Socket.IO do Node (`table:updated`, `order:updated`, ...).

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    /// `table:updated` `{ tableId, action }`
    TableUpdated { table_id: String, action: &'static str },
    /// `order:updated` `{ orderId, action }`
    OrderUpdated { order_id: String, action: &'static str },
    /// `cash:updated`
    CashUpdated,
    /// `kds:new_order`: itens novos para a cozinha/bar (o servidor monta o payload completo pelos ids)
    KdsNewOrder { order_id: String, item_ids: Vec<String> },
    /// `kds:item_updated` `{ itemId, status, station, tableId, orderId }`
    KdsItemUpdated { item_id: String, status: String, station: String, table_id: Option<String>, order_id: String },
    /// `kds:item_ready`: item pronto, avisa o garçom
    KdsItemReady {
        item_id: String,
        order_id: String,
        order_number: i64,
        /// name || "Mesa N" || "Balcão"
        table_name: String,
        table_number: Option<i64>,
        waiter_id: Option<String>,
        /// waiterName || "Garçom"
        waiter_name: String,
        station: String,
        product_name: String,
        quantity: i64,
        notes: Option<String>,
        /// ISO 8601 (toISOString)
        ready_at: String,
    },
    /// `kds:batch_updated` `{ orderId, station, status }` (station ausente = todas)
    KdsBatchUpdated { order_id: String, station: Option<String>, status: String },
    /// `kds:order_ready`: comanda pronta, avisa o garçom
    KdsOrderReady {
        order_id: String,
        order_number: i64,
        table_name: String,
        table_number: Option<i64>,
        waiter_id: Option<String>,
        waiter_name: String,
        /// station || "COZINHA"
        station: String,
        items: Vec<KdsReadyItem>,
        ready_at: String,
    },
    /// `catalog:updated` `{ scope }`: o cadastro mudou ("products", "categories", "modifiers", "promotions",
    /// "images"). O app atualiza o PDV e agenda a publicação do cardápio digital. O Node não emitia nada,
    /// então os APKs atuais ignoram.
    CatalogUpdated { scope: &'static str },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KdsReadyItem {
    pub name: String,
    pub quantity: i64,
    pub notes: Option<String>,
}

impl Event {
    /// Nome do evento no Socket.IO.
    pub fn socket_name(&self) -> &'static str {
        match self {
            Event::TableUpdated { .. } => "table:updated",
            Event::OrderUpdated { .. } => "order:updated",
            Event::CashUpdated => "cash:updated",
            Event::KdsNewOrder { .. } => "kds:new_order",
            Event::KdsItemUpdated { .. } => "kds:item_updated",
            Event::KdsItemReady { .. } => "kds:item_ready",
            Event::KdsBatchUpdated { .. } => "kds:batch_updated",
            Event::KdsOrderReady { .. } => "kds:order_ready",
            Event::CatalogUpdated { .. } => "catalog:updated",
        }
    }

    pub fn table(id: &str, action: &'static str) -> Self {
        Event::TableUpdated { table_id: id.to_string(), action }
    }

    pub fn order(id: &str, action: &'static str) -> Self {
        Event::OrderUpdated { order_id: id.to_string(), action }
    }
}
