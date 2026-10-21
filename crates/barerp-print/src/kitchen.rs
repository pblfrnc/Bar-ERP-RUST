//! Comanda de produção (cozinha / bar).
//! Tradução de `client/src/components/KitchenTicketReceipt.tsx`.

use chrono::NaiveDateTime;

use crate::escpos::{Align, EscPos};
use crate::format::time_seconds;

#[derive(Debug, Clone, PartialEq)]
pub struct KitchenItem {
    pub name: String,
    pub quantity: i64,
    /// Opções + observação, já montadas pelo servidor (`kitchenNote`).
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KitchenTicket {
    pub order_number: i64,
    pub table_name: Option<String>,
    pub table_number: Option<i64>,
    pub waiter_name: Option<String>,
    /// Hora do pedido (quem chama usa "agora" quando não tiver).
    pub created_at: NaiveDateTime,
    /// COZINHA, BAR ou TODOS; vazio imprime "PEDIDO COZINHA".
    pub station: Option<String>,
    pub items: Vec<KitchenItem>,
}

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Escreve a comanda em `doc`, terminando com avanço e corte.
/// Sem itens não escreve nada e devolve `false`, como o componente que não renderiza.
pub fn kitchen_ticket(doc: &mut EscPos, ticket: &KitchenTicket) -> bool {
    if ticket.items.is_empty() {
        return false;
    }

    doc.align(Align::Center).bold(true).double(true);
    doc.line(&format!(
        "*** {} ***",
        non_empty(&ticket.station)
            .unwrap_or("PEDIDO COZINHA")
            .to_uppercase()
    ));
    let table = match (non_empty(&ticket.table_name), ticket.table_number) {
        (Some(name), _) => name.to_uppercase(),
        (None, Some(n)) => format!("MESA {n}"),
        (None, None) => "BALCÃO".to_string(),
    };
    doc.invert(true).line(&format!(" {table} ")).invert(false);
    doc.double(false).align(Align::Left);
    doc.row(
        &format!("COMANDA: #{}", ticket.order_number),
        &format!("HORA: {}", time_seconds(ticket.created_at)),
    );
    doc.bold(false);
    doc.line(&format!(
        "ATENDENTE: {}",
        non_empty(&ticket.waiter_name).unwrap_or("Garçom")
    ));
    doc.divider('-');

    for item in &ticket.items {
        let qty = format!("{}x ", item.quantity);
        let indent = " ".repeat(qty.chars().count());
        doc.bold(true).size(false, true);
        doc.wrapped(&qty, &indent, &item.name.to_uppercase());
        doc.size(false, false);
        if let Some(notes) = non_empty(&item.notes) {
            // Observação em destaque obrigatório para a cozinha
            doc.wrapped(
                &format!("{indent}>> OBS: "),
                &format!("{indent}   "),
                &notes.to_uppercase(),
            );
        }
        doc.bold(false);
    }

    doc.divider('-');
    let total: i64 = ticket.items.iter().map(|i| i.quantity).sum();
    doc.align(Align::Center);
    doc.bold(true)
        .line(&format!("TOTAL DE ITENS: {total}"))
        .bold(false);
    doc.divider('=');
    doc.line("SISTEMA BAR ERP • IMPRESSÃO AUTOMÁTICA");
    doc.align(Align::Left).feed(2).cut(0);
    true
}
