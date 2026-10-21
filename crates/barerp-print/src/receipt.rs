//! Comprovante de consumo / conferência de mesa (não fiscal).
//! Tradução de `client/src/components/ThermalReceipt.tsx`.

use chrono::NaiveDateTime;

use crate::escpos::{Align, EscPos};
use crate::format::{cnpj, date_time, money};

/// Cabeçalho do estabelecimento (Configurações Fiscais).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Establishment {
    pub nome_fantasia: Option<String>,
    pub razao_social: Option<String>,
    pub cnpj: Option<String>,
    pub endereco: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptModifier {
    pub name: String,
    pub price: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptItem {
    pub name: String,
    pub quantity: i64,
    pub unit_price: f64,
    pub total_price: f64,
    /// Adicionais escolhidos (o preço já está somado ao `unit_price`).
    pub modifiers: Vec<ReceiptModifier>,
    pub promotion_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptPayment {
    /// CASH, CREDIT_CARD, DEBIT_CARD, PIX, VOUCHER, CREDIT_TAB
    pub method: String,
    pub amount: f64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptOrder {
    pub order_number: i64,
    /// OPEN, PAID, CLOSED, CANCELLED
    pub status: String,
    pub created_at: NaiveDateTime,
    pub closed_at: Option<NaiveDateTime>,
    pub table_name: Option<String>,
    pub table_number: Option<i64>,
    pub waiter_name: Option<String>,
    pub customer_name: Option<String>,
    pub items: Vec<ReceiptItem>,
    pub subtotal: f64,
    pub is_service_fee_active: bool,
    pub service_fee: f64,
    /// 0.10 = 10%
    pub service_fee_rate: f64,
    pub discount: f64,
    pub total: f64,
    /// Já pago, inclusive abatimentos parciais (haver).
    pub paid_amount: f64,
    pub payments: Vec<ReceiptPayment>,
}

impl ReceiptOrder {
    pub fn is_closed(&self) -> bool {
        self.status == "PAID" || self.status == "CLOSED"
    }

    fn table_label(&self) -> String {
        match (&self.table_name, self.table_number) {
            (Some(name), _) if !name.is_empty() => name.clone(),
            (_, Some(n)) if n != 0 => format!("Mesa {n}"),
            _ => "Balcão".to_string(),
        }
    }
}

/// Nome da forma de pagamento como aparece para o cliente.
pub fn payment_label(method: &str) -> &str {
    match method {
        "CASH" => "Dinheiro",
        "CREDIT_CARD" => "Cartão Crédito",
        "DEBIT_CARD" => "Cartão Débito",
        "PIX" => "PIX",
        "VOUCHER" => "Vale",
        "CREDIT_TAB" => "Fiado",
        other => other,
    }
}

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Escreve o comprovante inteiro em `doc`, terminando com avanço e corte.
/// `printed_at` é a hora do rodapé "Impresso em".
pub fn consumption_receipt(
    doc: &mut EscPos,
    order: &ReceiptOrder,
    establishment: Option<&Establishment>,
    printed_at: NaiveDateTime,
) {
    let closed = order.is_closed();
    let order_date = if closed {
        order.closed_at.unwrap_or(order.created_at)
    } else {
        order.created_at
    };

    // Cabeçalho com os dados reais do estabelecimento
    if let Some(est) = establishment {
        let fantasia = non_empty(&est.nome_fantasia);
        let razao = non_empty(&est.razao_social);
        doc.align(Align::Center);
        if let Some(name) = fantasia.or(razao) {
            doc.bold(true)
                .size(false, true)
                .line(&name.to_uppercase())
                .size(false, false)
                .bold(false);
        }
        if let (Some(_), Some(razao)) = (fantasia, razao) {
            doc.line(razao);
        }
        if let Some(c) = non_empty(&est.cnpj) {
            doc.line(&format!("CNPJ: {}", cnpj(c)));
        }
        if let Some(end) = non_empty(&est.endereco) {
            doc.line(end);
        }
        doc.align(Align::Left);
    }

    doc.divider('-');
    doc.align(Align::Center).bold(true);
    doc.line(if closed {
        "COMPROVANTE DE CONSUMO"
    } else {
        "CONFERÊNCIA DE MESA"
    });
    doc.bold(false)
        .line("DOCUMENTO SEM VALOR FISCAL")
        .align(Align::Left);
    doc.divider('-');

    doc.row(
        &format!("MESA: {}", order.table_label()),
        &format!("CMD: #{}", order.order_number),
    );
    doc.line(&format!(
        "{}: {}",
        if closed { "FECHADA" } else { "ABERTA" },
        date_time(order_date)
    ));
    doc.line(&format!(
        "GARÇOM: {}",
        non_empty(&order.waiter_name).unwrap_or("Atendente")
    ));
    if let Some(c) = non_empty(&order.customer_name) {
        doc.line(&format!("CLIENTE: {c}"));
    }

    doc.divider('-');
    doc.bold(true).row("ITEM", "TOTAL").bold(false);
    for item in &order.items {
        doc.bold(true).line(&item.name).bold(false);
        for m in &item.modifiers {
            let price = if m.price > 0.0 {
                format!(" ({})", money(m.price))
            } else {
                String::new()
            };
            doc.wrapped("  + ", "    ", &format!("{}{price}", m.name));
        }
        if let Some(p) = non_empty(&item.promotion_name) {
            doc.wrapped("  * ", "    ", p);
        }
        doc.row(
            &format!("{} x {}", item.quantity, money(item.unit_price)),
            &money(item.total_price),
        );
    }

    doc.divider('-');
    doc.row("SUBTOTAL:", &format!("R$ {}", money(order.subtotal)));
    if order.is_service_fee_active && order.service_fee > 0.0 {
        let pct = (order.service_fee_rate * 100.0).round() as i64;
        doc.row(
            &format!("TX SERVIÇO ({pct}%):"),
            &format!("R$ {}", money(order.service_fee)),
        );
    }
    if order.discount > 0.0 {
        doc.row("DESCONTO:", &format!("- R$ {}", money(order.discount)));
    }
    doc.bold(true).size(false, true);
    doc.row("TOTAL:", &format!("R$ {}", money(order.total)));
    doc.size(false, false);
    if order.paid_amount > 0.0 {
        doc.row("VALOR PAGO:", &format!("R$ {}", money(order.paid_amount)));
    }
    let remaining = order.total - order.paid_amount;
    if remaining > 0.009 {
        doc.row("SALDO RESTANTE:", &format!("R$ {}", money(remaining)));
    }
    doc.bold(false);

    if !order.payments.is_empty() {
        doc.divider('-');
        doc.bold(true).line("PAGAMENTOS:").bold(false);
        for p in &order.payments {
            let label = match non_empty(&p.notes) {
                Some(n) => format!("{} ({n})", payment_label(&p.method)),
                None => payment_label(&p.method).to_string(),
            };
            doc.row(&label, &format!("R$ {}", money(p.amount)));
        }
    }

    doc.divider('-');
    doc.align(Align::Center);
    doc.line("Agradecemos a preferência! Volte sempre!");
    doc.line(&format!("Impresso em {}", date_time(printed_at)));
    doc.align(Align::Left).feed(2).cut(0);
}
