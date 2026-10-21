//! Comandas Finalizadas & Histórico (OrderHistoryModal): busca, filtro por dia, detalhe e 2ª via.

use chrono::{Duration, Local, NaiveDate};
use slint::{ComponentHandle, SharedString};

use barerp_core::money::brl;
use barerp_core::pricing::ModifierSnapshot;
use barerp_db::order_history::{self, HistoryFilter};
use barerp_db::orders::{self, OrderDetail, OrderHead};

use super::{modal, with_state};
use crate::ctx::AppCtx;
use crate::view::model;
use crate::{DetailLine, HistoryRow, MesasModal, MesasModalStore, PayLine};

thread_local! {
    static ROWS: std::cell::RefCell<(Vec<OrderHead>, Option<String>)> = const { std::cell::RefCell::new((Vec::new(), None)) };
}

fn place(h: &OrderHead) -> String {
    match (&h.table_name, h.table_number) {
        (Some(n), _) if !n.is_empty() => n.clone(),
        (_, Some(n)) => format!("Mesa {n}"),
        _ => "Balcão / Direto".into(),
    }
}

fn is_loss(h: &OrderHead) -> bool {
    h.status == "CLOSED" && h.paid_amount < h.total - 0.005
}

fn when(h: &OrderHead) -> String {
    h.closed_at.unwrap_or(h.created_at).0.with_timezone(&Local).format("%d/%m/%Y %H:%M").to_string()
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

fn render_rows(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    let term = m.get_h_search().trim().to_lowercase();
    ROWS.with(|r| {
        let (rows, selected) = &*r.borrow();
        m.set_h_total_count(rows.len() as i32);
        let items: Vec<HistoryRow> = rows
            .iter()
            .filter(|h| {
                term.is_empty()
                    || h.order_number.to_string().contains(&term)
                    || h.table_number.is_some_and(|n| n.to_string().contains(&term))
                    || h.table_name.as_deref().is_some_and(|n| n.to_lowercase().contains(&term))
                    || h.customer_name.as_deref().is_some_and(|n| n.to_lowercase().contains(&term))
                    || h.waiter_name.as_deref().is_some_and(|n| n.to_lowercase().contains(&term))
            })
            .map(|h| {
                let mut info = vec![h.waiter_name.clone().unwrap_or_else(|| "Garçom".into())];
                if h.discount > 0.0 {
                    info.push(format!("Desc. {}", brl(h.discount)));
                }
                HistoryRow {
                    id: h.id.clone().into(),
                    number: format!("#{}", h.order_number).into(),
                    place: place(h).into(),
                    when: when(h).into(),
                    info: info.join(" · ").into(),
                    total: brl(h.total).into(),
                    loss: is_loss(h),
                    selected: selected.as_deref() == Some(h.id.as_str()),
                }
            })
            .collect();
        m.set_h_rows(model(items));
    });
}

fn render_detail(ui: &crate::AppWindow, d: &OrderDetail) {
    let m = ui.global::<MesasModalStore>();
    let h = &d.head;
    m.set_h_has_selection(true);
    m.set_h_number(format!("#{}", h.order_number).into());
    m.set_h_place(place(h).into());
    m.set_h_when(when(h).into());
    m.set_h_waiter(h.waiter_name.clone().unwrap_or_else(|| "Garçom".into()).into());
    m.set_h_customer(h.customer_name.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "Não identificado".into()).into());
    m.set_h_loss(is_loss(h));
    let mut items = d.items.clone();
    items.reverse();
    let lines: Vec<DetailLine> = items
        .iter()
        .map(|l| DetailLine {
            id: l.id.clone().into(),
            quantity: format!("{}", l.quantity).into(),
            name: l.product_name.clone().into(),
            kds: SharedString::new(),
            modifiers: l
                .modifiers
                .as_deref()
                .and_then(|m| serde_json::from_str::<Vec<ModifierSnapshot>>(m).ok())
                .map(|v| v.into_iter().map(|m| m.name).collect::<Vec<_>>().join(" · "))
                .unwrap_or_default()
                .into(),
            promotion: l.promotion_name.clone().unwrap_or_default().into(),
            notes: l.notes.clone().unwrap_or_default().into(),
            unit_info: brl(l.unit_price).into(),
            total: brl(l.total_price).into(),
        })
        .collect();
    m.set_h_lines(model(lines));
    m.set_h_subtotal(brl(h.subtotal).into());
    m.set_h_fee(brl(if h.is_service_fee_active { h.service_fee } else { 0.0 }).into());
    m.set_h_discount(format!("- {}", brl(h.discount)).into());
    m.set_h_final(brl(h.total).into());
    let pays: Vec<PayLine> = d
        .payments
        .iter()
        .map(|p| PayLine {
            when: format!(
                "{} · {}{}",
                p.received_at.0.with_timezone(&Local).format("%H:%M"),
                method_label(&p.method),
                p.notes.as_deref().map(|n| format!(" · {n}")).unwrap_or_default()
            )
            .into(),
            amount: brl(p.amount).into(),
        })
        .collect();
    m.set_h_payments(model(pays));
}

fn select(ctx: &AppCtx, id: String) {
    ROWS.with(|r| r.borrow_mut().1 = Some(id.clone()));
    if let Some(ui) = ctx.ui() {
        render_rows(&ui);
    }
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { orders::detail(&db, &id).await }, move |ui, result| match result {
        Ok(d) => render_detail(ui, &d),
        Err(e) => next.status(format!("Erro ao abrir a comanda: {e}"), true),
    });
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%d/%m/%Y").ok()
}

fn load(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    let date_text = m.get_h_date().to_string();
    let date = parse_date(&date_text);
    if !date_text.trim().is_empty() && date.is_none() {
        ctx.status("Data inválida: use dd/mm/aaaa.", true);
        return;
    }
    m.set_h_loading(true);
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(
        async move { order_history::finished_orders(&db, &HistoryFilter { date, search: String::new(), limit: 100 }).await },
        move |ui, result| {
            ui.global::<MesasModalStore>().set_h_loading(false);
            match result {
                Ok(rows) => {
                    let first = rows.first().map(|h| h.id.clone());
                    let keep = ROWS.with(|r| {
                        let mut r = r.borrow_mut();
                        let keep = r.1.clone().filter(|id| rows.iter().any(|h| &h.id == id));
                        r.0 = rows;
                        r.1 = keep.clone();
                        keep
                    });
                    render_rows(ui);
                    match keep.or(first) {
                        Some(id) => select(&next, id),
                        None => ui.global::<MesasModalStore>().set_h_has_selection(false),
                    }
                }
                Err(e) => next.status(format!("Erro ao carregar o histórico: {e}"), true),
            }
        },
    );
}

pub(crate) fn open(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    m.set_h_search(SharedString::new());
    m.set_h_date(SharedString::new());
    m.set_h_has_selection(false);
    ROWS.with(|r| *r.borrow_mut() = (Vec::new(), None));
    m.set_h_rows(model(Vec::new()));
    modal(&ui, MesasModal::History);
    with_state(|_| ());
    load(ctx);
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    m.on_h_filter({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render_rows(&ui);
            }
        }
    });
    m.on_h_refresh({
        let ctx = ctx.clone();
        move || load(&ctx)
    });
    m.on_h_select({
        let ctx = ctx.clone();
        move |id| select(&ctx, id.to_string())
    });
    m.on_h_date_quick({
        let ctx = ctx.clone();
        move |which| {
            let Some(ui) = ctx.ui() else { return };
            let today = Local::now().date_naive();
            let text = match which.as_str() {
                "today" => today.format("%d/%m/%Y").to_string(),
                "yesterday" => (today - Duration::days(1)).format("%d/%m/%Y").to_string(),
                _ => String::new(),
            };
            ui.global::<MesasModalStore>().set_h_date(text.into());
            load(&ctx);
        }
    });
    m.on_h_print({
        let ctx = ctx.clone();
        move || {
            let Some(id) = ROWS.with(|r| r.borrow().1.clone()) else { return };
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { crate::print::print_order_receipt(&db, &id).await }, move |_, result| match result {
                Ok(message) => next.status(message, false),
                Err(e) => next.status(format!("Não foi possível imprimir: {e}"), true),
            });
        }
    });
}
