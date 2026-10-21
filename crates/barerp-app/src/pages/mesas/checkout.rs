//! Fechar conta (CheckoutModal): resumo, 10%, desconto, racha-conta (igual, por itens, abater valor), formas de
//! pagamento, fiado com cliente, troco, avisos e WhatsApp.

use std::collections::HashMap;
use std::time::Duration;

use chrono::Local;
use slint::{ComponentHandle, SharedString};

use barerp_core::money::brl;
use barerp_core::pricing::{round2, ModifierSnapshot};
use barerp_db::customers::{self, NewCustomer};
use barerp_db::orders::{self, NewPayment, OrderDetail, PaidItemLine, ProcessPaymentInput};

use super::{card, close_modal, modal, modal_error, refresh, table_title, with_state};
use crate::ctx::AppCtx;
use crate::view::model;
use crate::{Choice, MesasModal, MesasModalStore, PayLine, SplitItem};

const TOLERANCE: f64 = 0.05;

#[derive(Default)]
pub(crate) struct Checkout {
    detail: Option<OrderDetail>,
    split_count: i32,
    /// item → quantidade escolhida no "Por Itens"
    selected: HashMap<String, i64>,
}

fn method_label(method: &str) -> &str {
    match method {
        "CASH" => "Dinheiro",
        "PIX" => "PIX",
        "CREDIT_CARD" => "Cartão de Crédito",
        "DEBIT_CARD" => "Cartão de Débito",
        "VOUCHER" => "Voucher",
        "CREDIT_TAB" => "Fiado",
        other => other,
    }
}

fn parse_money(text: &str) -> Option<f64> {
    let t = text.trim().replace("R$", "").replace(' ', "");
    let t = if t.contains(',') { t.replace('.', "").replace(',', ".") } else { t };
    t.parse::<f64>().ok()
}

fn money_text(value: f64) -> String {
    format!("{value:.2}").replace('.', ",")
}

fn remaining(d: &OrderDetail) -> f64 {
    (d.head.total - d.head.paid_amount).max(0.0)
}

fn modifiers_label(json: Option<&str>) -> String {
    json.and_then(|m| serde_json::from_str::<Vec<ModifierSnapshot>>(m).ok())
        .map(|l| l.into_iter().map(|m| m.name).collect::<Vec<_>>().join(" · "))
        .unwrap_or_default()
}

/// Soma dos itens escolhidos no "Por Itens" (com 10% se estiver ligado).
fn selected_total(c: &Checkout) -> f64 {
    let Some(d) = &c.detail else { return 0.0 };
    let subtotal: f64 = c
        .selected
        .iter()
        .filter_map(|(id, qty)| d.items.iter().find(|i| &i.id == id).map(|i| i.unit_price * *qty as f64))
        .sum();
    let fee = if d.head.is_service_fee_active { subtotal * d.head.service_fee_rate } else { 0.0 };
    round2(subtotal + fee)
}

/// Redesenha tudo que depende da comanda e do que foi digitado.
fn render(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    with_state(|s| {
        let c = &s.checkout;
        let Some(d) = &c.detail else { return };
        let h = &d.head;
        let rem = remaining(d);
        m.set_c_order(format!("Comanda #{}", h.order_number).into());
        m.set_c_items(format!("Subtotal ({} itens):", d.items.len()).into());
        m.set_c_subtotal(brl(h.subtotal).into());
        m.set_c_fee_active(h.is_service_fee_active);
        m.set_c_fee(brl(if h.is_service_fee_active { h.service_fee } else { 0.0 }).into());
        m.set_c_total(brl(h.total).into());
        m.set_c_remaining(brl(rem).into());
        m.set_c_paid(if h.paid_amount > 0.0 { format!("- {}", brl(h.paid_amount)).into() } else { SharedString::new() });
        let mut payments: Vec<PayLine> = d
            .payments
            .iter()
            .map(|p| {
                let time = p.received_at.0.with_timezone(&Local).format("%H:%M");
                let notes = p.notes.as_deref().map(|n| format!(" · {n}")).unwrap_or_default();
                PayLine { when: format!("{time} · {}{notes}", method_label(&p.method)).into(), amount: brl(p.amount).into() }
            })
            .collect();
        payments.reverse();
        m.set_c_payments(model(payments));

        m.set_c_split_count(c.split_count);
        m.set_c_per_person(brl(rem / f64::from(c.split_count.max(1))).into());
        let items: Vec<SplitItem> = d
            .items
            .iter()
            .filter(|i| i.quantity - i.paid_quantity > 0)
            .map(|i| SplitItem {
                id: i.id.clone().into(),
                name: i.product_name.clone().into(),
                modifiers: modifiers_label(i.modifiers.as_deref()).into(),
                info: format!("{}x {}", i.quantity - i.paid_quantity, brl(i.unit_price)).into(),
                selected: c.selected.get(&i.id).copied().unwrap_or(0) as i32,
            })
            .collect();
        m.set_c_split_items(model(items));
        let sel = selected_total(c);
        m.set_c_split_total(if sel > 0.0 { brl(sel).into() } else { SharedString::new() });
        let shortcuts: Vec<i32> = [10, 20, 50, 100].into_iter().filter(|v| f64::from(*v) < rem).collect();
        m.set_c_amount_shortcuts(model(shortcuts));

        // Valor, troco e avisos
        let to_pay = parse_money(&m.get_c_amount()).unwrap_or(0.0);
        let mode = m.get_c_split_mode();
        m.set_c_amount_preview(if mode == "amount" && to_pay > 0.0 && to_pay <= rem + TOLERANCE {
            format!("Abatendo {} → restará {}", brl(to_pay), brl((rem - to_pay).max(0.0))).into()
        } else {
            SharedString::new()
        });
        let cash = parse_money(&m.get_c_cash()).unwrap_or(0.0);
        m.set_c_change(if m.get_c_method() == "CASH" && cash > 0.0 { brl((cash - to_pay).max(0.0)).into() } else { SharedString::new() });
        m.set_c_over_warning(if to_pay > rem + TOLERANCE {
            format!(
                "O valor é maior que o saldo restante ({}). Para dinheiro, informe o troco em \"Valor Entregue\".",
                brl(rem)
            )
            .into()
        } else {
            SharedString::new()
        });
        let partial = to_pay < rem - TOLERANCE;
        m.set_c_partial(partial);
        m.set_c_partial_warning(if partial {
            format!("Pagamento parcial: a mesa continuará ocupada com saldo restante de {}.", brl(rem - to_pay)).into()
        } else {
            SharedString::new()
        });
        m.set_c_button(
            if partial {
                if mode == "amount" {
                    format!("ABATER {} DA CONTA", brl(to_pay))
                } else {
                    format!("REGISTRAR PAGAMENTO PARCIAL ({})", brl(to_pay))
                }
            } else {
                format!("CONFIRMAR PAGAMENTO & LIBERAR MESA ({})", brl(to_pay))
            }
            .into(),
        );
    });
}

fn set_amount(ui: &crate::AppWindow, value: f64) {
    ui.global::<MesasModalStore>().set_c_amount(money_text(value).into());
}

/// Recarrega a comanda (depois de 10% / desconto) e põe o saldo no valor a pagar.
fn reload(ctx: &AppCtx, reset_amount: bool) {
    let Some(order_id) = with_state(|s| s.checkout.detail.as_ref().map(|d| d.head.id.clone())) else { return };
    let db = ctx.db.clone();
    ctx.spawn(async move { orders::detail(&db, &order_id).await }, move |ui, result| match result {
        Ok(detail) => {
            let rem = remaining(&detail);
            with_state(|s| s.checkout.detail = Some(detail));
            if reset_amount {
                set_amount(ui, rem);
            }
            render(ui);
        }
        Err(e) => modal_error(ui, e.to_string()),
    });
}

/// Abre o fechamento da mesa em foco.
pub(crate) fn open(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let Some(table_id) = with_state(|s| s.table_id.clone()) else { return };
    let Some(card) = card(&table_id) else { return };
    let Some(order_id) = card.order_id.clone() else {
        ctx.status("Esta mesa não tem comanda aberta.", true);
        return;
    };
    let m = ui.global::<MesasModalStore>();
    let mut title = table_title(&card);
    if let Some(customer) = card.customer_name.clone().filter(|c| !c.is_empty()) {
        title.push_str(&format!(" • {customer}"));
    }
    m.set_c_title(title.into());
    m.set_c_split_mode("simple".into());
    m.set_c_method("PIX".into());
    m.set_c_discount(SharedString::new());
    m.set_c_cash(SharedString::new());
    m.set_c_customer_id(SharedString::new());
    m.set_c_new_customer(SharedString::new());
    m.set_c_success(false);
    with_state(|s| {
        s.checkout = Checkout { detail: None, split_count: card.customer_count.max(2) as i32, selected: HashMap::new() };
    });

    let db = ctx.db.clone();
    ctx.spawn(
        async move {
            let detail = orders::detail(&db, &order_id).await?;
            let customers = customers::list(&db).await.unwrap_or_default();
            barerp_db::DbResult::Ok((detail, customers))
        },
        |ui, result| match result {
            Ok((detail, customers)) => {
                let rem = remaining(&detail);
                with_state(|s| s.checkout.detail = Some(detail));
                let choices: Vec<Choice> = customers
                    .into_iter()
                    .map(|c| Choice {
                        id: c.id.into(),
                        label: c.name.into(),
                        detail: format!("Saldo: {}", brl(c.credit_tab_balance)).into(),
                        selected: false,
                    })
                    .collect();
                ui.global::<MesasModalStore>().set_c_customers(model(choices));
                set_amount(ui, rem);
                render(ui);
                modal(ui, MesasModal::Checkout);
            }
            Err(e) => {
                modal(ui, MesasModal::None);
                if let Some(ui) = Some(ui) {
                    ui.set_status_message(format!("Erro ao abrir o fechamento: {e}").into());
                    ui.set_status_error(true);
                }
            }
        },
    );
}

/// Texto da conferência para o WhatsApp, igual ao do original.
fn whatsapp_text(d: &OrderDetail, title: &str, split: i32) -> String {
    let h = &d.head;
    let mut msg = format!("*CONFERÊNCIA DE MESA - {title}*\nComanda #{}\n--------------------------------\n", h.order_number);
    let mut items = d.items.clone();
    items.reverse();
    for it in &items {
        msg.push_str(&format!("{}x {} - R$ {:.2}\n", it.quantity, it.product_name, it.total_price));
    }
    msg.push_str("--------------------------------\n");
    msg.push_str(&format!("Subtotal: R$ {:.2}\n", h.subtotal));
    if h.is_service_fee_active {
        msg.push_str(&format!("Taxa Serviço (10%): R$ {:.2}\n", h.service_fee));
    }
    if h.discount > 0.0 {
        msg.push_str(&format!("Desconto: - R$ {:.2}\n", h.discount));
    }
    msg.push_str(&format!("*TOTAL: R$ {:.2}*\n", h.total));
    if split > 1 {
        msg.push_str(&format!("Divisão ({split} pessoas): R$ {:.2} cada\n", h.total / f64::from(split)));
    }
    msg.push_str("\nObrigado pela preferência!");
    msg
}

fn url_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn confirm(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    let Some((order_id, rem, items)) = with_state(|s| {
        let d = s.checkout.detail.as_ref()?;
        Some((d.head.id.clone(), remaining(d), d.items.clone()))
    }) else {
        return;
    };
    let amount = parse_money(&m.get_c_amount()).unwrap_or(0.0);
    if amount <= 0.0 {
        modal_error(&ui, "Informe um valor válido para pagamento");
        return;
    }
    if amount > rem + TOLERANCE {
        modal_error(&ui, format!("O valor informado é maior que o saldo restante ({}).", brl(rem)));
        return;
    }
    let method = m.get_c_method().to_string();
    let mode = m.get_c_split_mode().to_string();
    let customer_id = m.get_c_customer_id().to_string();
    let new_customer = m.get_c_new_customer().trim().to_string();
    if method == "CREDIT_TAB" && customer_id.is_empty() && new_customer.is_empty() {
        modal_error(&ui, "Escolha o cliente ou digite o nome de um novo cliente para lançar no fiado.");
        return;
    }
    let closing = amount >= rem - TOLERANCE;
    let split_count = with_state(|s| s.checkout.split_count);
    let selected: Vec<(String, i64)> =
        with_state(|s| s.checkout.selected.iter().filter(|(_, q)| **q > 0).map(|(id, q)| (id.clone(), *q)).collect());
    let notes = match mode.as_str() {
        "items" => Some(format!(
            "Itens: {}",
            selected
                .iter()
                .map(|(id, q)| format!("{q}x {}", items.iter().find(|i| &i.id == id).map(|i| i.product_name.as_str()).unwrap_or("Item")))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        "amount" => Some(if closing { "Haver (quitação)" } else { "Haver (abatimento parcial)" }.to_string()),
        _ => (split_count > 1).then(|| format!("Divisão ({split_count}p)")),
    };
    let paid_items = (mode == "items")
        .then(|| selected.iter().map(|(id, q)| PaidItemLine { item_id: id.clone(), quantity: *q }).collect::<Vec<_>>());

    m.set_busy(true);
    m.set_error(SharedString::new());
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(
        async move {
            let mut customer = (!customer_id.is_empty()).then_some(customer_id);
            if method == "CREDIT_TAB" && customer.is_none() {
                let created = customers::create(&db, NewCustomer { name: new_customer, phone: None, document: None, notes: None }).await?;
                customer = Some(created.id);
            }
            let input = ProcessPaymentInput {
                payments: vec![NewPayment { amount: round2(amount), method, notes }],
                close_order: Some(closing),
                customer_id: if customer.is_some() { customer } else { None },
                paid_items,
            };
            orders::process_payment(&db, &order_id, input).await
        },
        move |ui, result| match result {
            Ok((_, events)) => {
                next.emit(&events);
                let m = ui.global::<MesasModalStore>();
                m.set_busy(false);
                m.set_c_success(true);
                let after = next.clone();
                slint::Timer::single_shot(Duration::from_millis(1200), move || {
                    if let Some(ui) = after.ui() {
                        if ui.global::<MesasModalStore>().get_modal() == MesasModal::Checkout {
                            close_modal(&ui);
                        }
                    }
                    refresh(&after);
                });
            }
            Err(e) => modal_error(ui, e.to_string()),
        },
    );
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();

    m.on_c_changed({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render(&ui);
            }
        }
    });
    m.on_c_toggle_fee({
        let ctx = ctx.clone();
        move || {
            let Some(order_id) = with_state(|s| s.checkout.detail.as_ref().map(|d| d.head.id.clone())) else { return };
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { orders::set_service_fee(&db, &order_id, None, None).await }, move |ui, result| match result {
                Ok(events) => {
                    next.emit(&events);
                    reload(&next, true);
                    refresh(&next);
                }
                Err(e) => modal_error(ui, e.to_string()),
            });
        }
    });
    m.on_c_apply_discount({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let value = parse_money(&ui.global::<MesasModalStore>().get_c_discount()).unwrap_or(0.0);
            let Some(order_id) = with_state(|s| s.checkout.detail.as_ref().map(|d| d.head.id.clone())) else { return };
            let db = ctx.db.clone();
            let next = ctx.clone();
            let user = ctx.user_name();
            ctx.spawn(
                async move { orders::set_discount(&db, &order_id, value, user.as_deref()).await },
                move |ui, result| match result {
                    Ok(events) => {
                        next.emit(&events);
                        ui.global::<MesasModalStore>().set_error(SharedString::new());
                        reload(&next, true);
                        refresh(&next);
                    }
                    Err(e) => modal_error(ui, e.to_string()),
                },
            );
        }
    });
    m.on_c_set_split({
        let ctx = ctx.clone();
        move |mode| {
            let Some(ui) = ctx.ui() else { return };
            let m = ui.global::<MesasModalStore>();
            let previous = m.get_c_split_mode();
            m.set_c_split_mode(mode.clone());
            let (rem, sel) = with_state(|s| (s.checkout.detail.as_ref().map(remaining).unwrap_or(0.0), selected_total(&s.checkout)));
            match mode.as_str() {
                "amount" => m.set_c_amount(SharedString::new()),
                "items" => set_amount(&ui, sel.min(rem)),
                _ if previous == "amount" => set_amount(&ui, rem),
                _ => {}
            }
            render(&ui);
        }
    });
    m.on_c_set_count({
        let ctx = ctx.clone();
        move |n| {
            let Some(ui) = ctx.ui() else { return };
            let rem = with_state(|s| {
                s.checkout.split_count = n;
                s.checkout.detail.as_ref().map(remaining).unwrap_or(0.0)
            });
            set_amount(&ui, rem / f64::from(n.max(1)));
            render(&ui);
        }
    });
    m.on_c_item_qty({
        let ctx = ctx.clone();
        move |index, delta| {
            let Some(ui) = ctx.ui() else { return };
            let (rem, sel) = with_state(|s| {
                let c = &mut s.checkout;
                if let Some(d) = &c.detail {
                    let unpaid: Vec<_> = d.items.iter().filter(|i| i.quantity - i.paid_quantity > 0).collect();
                    if let Some(item) = unpaid.get(index as usize) {
                        let max = item.quantity - item.paid_quantity;
                        let current = c.selected.get(&item.id).copied().unwrap_or(0);
                        let next = (current + i64::from(delta)).clamp(0, max);
                        let id = item.id.clone();
                        if next == 0 {
                            c.selected.remove(&id);
                        } else {
                            c.selected.insert(id, next);
                        }
                    }
                }
                (c.detail.as_ref().map(remaining).unwrap_or(0.0), selected_total(c))
            });
            // Com desconto na conta, a soma dos itens pode passar do saldo restante
            set_amount(&ui, sel.min(rem));
            render(&ui);
        }
    });
    m.on_c_shortcut({
        let ctx = ctx.clone();
        move |value| {
            if let Some(ui) = ctx.ui() {
                set_amount(&ui, f64::from(value));
                render(&ui);
            }
        }
    });
    m.on_c_all({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let rem = with_state(|s| s.checkout.detail.as_ref().map(remaining).unwrap_or(0.0));
            set_amount(&ui, rem);
            render(&ui);
        }
    });
    m.on_c_whatsapp({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let title = ui.global::<MesasModalStore>().get_c_title().to_string();
            let place = title.split(" • ").next().unwrap_or(&title).to_string();
            let Some(text) = with_state(|s| s.checkout.detail.as_ref().map(|d| whatsapp_text(d, &place, s.checkout.split_count)))
            else {
                return;
            };
            crate::ctx::open_external(&format!("https://api.whatsapp.com/send?text={}", url_encode(&text)));
        }
    });
    m.on_c_confirm({
        let ctx = ctx.clone();
        move || confirm(&ctx)
    });
}


/// Gerador de prints: abre o fechamento com a comanda já carregada.
pub(crate) fn preview(ui: &crate::AppWindow, detail: OrderDetail, mode: &str, method: &str, amount: &str) {
    let m = ui.global::<MesasModalStore>();
    m.set_c_title("Mesa 02 • Lucas & Amigos".into());
    m.set_c_split_mode(mode.into());
    m.set_c_method(method.into());
    m.set_c_amount(amount.into());
    m.set_c_cash(if method == "CASH" { "100,00".into() } else { SharedString::new() });
    m.set_c_success(false);
    m.set_c_customers(model(vec![
        Choice { id: "c1".into(), label: "João Silva".into(), detail: "Saldo: R$ 42,00".into(), selected: false },
        Choice { id: "c2".into(), label: "Maria Souza".into(), detail: "Saldo: R$ 0,00".into(), selected: false },
    ]));
    let first = detail.items.first().map(|i| i.id.clone());
    with_state(|s| {
        s.checkout = Checkout { detail: Some(detail), split_count: 2, selected: HashMap::new() };
        if mode == "items" {
            if let Some(id) = first {
                s.checkout.selected.insert(id, 1);
            }
        }
    });
    render(ui);
    modal(ui, MesasModal::Checkout);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_parsing() {
        assert_eq!(parse_money("66,00"), Some(66.0));
        assert_eq!(parse_money("R$ 1.234,50"), Some(1234.5));
        assert_eq!(parse_money("12.5"), Some(12.5));
        assert_eq!(parse_money(""), None);
        assert_eq!(money_text(66.0), "66,00");
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_encode("*TOTAL: R$ 1*\n"), "%2ATOTAL%3A%20R%24%201%2A%0A");
        assert_eq!(url_encode("ç"), "%C3%A7");
    }
}
