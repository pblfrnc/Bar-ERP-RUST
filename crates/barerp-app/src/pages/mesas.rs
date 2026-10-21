//! Página "Mesas & Salão": port de TablesView/TableCard e dos modais da área.
//! Ver `paridade/mesas.md` para a lista de funções do app original e o estado do port.

use std::cell::RefCell;

use chrono::Utc;
use slint::{ComponentHandle, Model, SharedString};

use barerp_core::money::brl;
use barerp_core::pricing::ModifierSnapshot;
use barerp_db::orders::{self, OpenTable, OrderDetail};
use barerp_db::table_admin::{self, TableInput};
use barerp_db::tables::{self, TableCard};
use barerp_db::{order_history, waiters};

use crate::ctx::AppCtx;
use crate::view::model;
use crate::{AppWindow, Choice, DetailLine, MesaCard, MesasModal, MesasModalStore, MesasStore, Page};

mod checkout;
mod equipe;
mod history;
mod launch;
mod manage;

/// Estado da página (só a thread da interface mexe).
#[derive(Default)]
pub(crate) struct State {
    pub cards: Vec<TableCard>,
    /// Mesa em foco nos modais
    pub table_id: Option<String>,
    pub detail: Option<OrderDetail>,
    /// Ação pendente de confirmação
    pub confirm: Option<Confirm>,
    pub launch: launch::Launch,
    pub checkout: checkout::Checkout,
}

#[derive(Clone)]
pub(crate) enum Confirm {
    Loss(String),
    RemoveItem { order_id: String, item_id: String },
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

pub(crate) fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// "42m" ou "1h 5m", como o cartão do original.
fn elapsed_short(card: &TableCard) -> String {
    let Some(opened) = card.opened_at else { return String::new() };
    let mins = (Utc::now() - opened.0).num_minutes().max(0);
    if mins < 60 { format!("{mins}m") } else { format!("{}h {}m", mins / 60, mins % 60) }
}

/// "42 min" ou "1h 5m", como o detalhe da mesa do original.
fn elapsed_long(card: &TableCard) -> String {
    let Some(opened) = card.opened_at else { return String::new() };
    let mins = (Utc::now() - opened.0).num_minutes().max(0);
    if mins < 60 { format!("{mins} min") } else { format!("{}h {}m", mins / 60, mins % 60) }
}

pub(crate) fn table_title(card: &TableCard) -> String {
    card.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Mesa {}", card.number))
}

fn haver_label(total: f64, paid: f64) -> String {
    if paid > 0.0 {
        format!("Haver {} · Falta {}", brl(paid), brl((total - paid).max(0.0)))
    } else {
        String::new()
    }
}

fn card_view(c: &TableCard) -> MesaCard {
    let total = c.total.unwrap_or(0.0);
    MesaCard {
        id: c.id.clone().into(),
        number: c.number as i32,
        name: c.name.clone().unwrap_or_default().into(),
        section: c.section.clone().into(),
        status: c.status.clone().into(),
        capacity: c.capacity as i32,
        customer: c.customer_name.clone().unwrap_or_default().into(),
        people: c.customer_count as i32,
        elapsed: elapsed_short(c).into(),
        items: c.line_count as i32,
        total: brl(total).into(),
        haver: haver_label(total, c.paid_amount.unwrap_or(0.0)).into(),
        fee: c.fee_active.unwrap_or(false),
    }
}

/// Atualiza indicadores, seções e a grade com os filtros da tela.
fn render(ui: &AppWindow, s: &State) {
    let store = ui.global::<MesasStore>();
    let cards = &s.cards;
    let total = cards.len();
    let occupied = cards.iter().filter(|c| c.status == "OCCUPIED").count();
    let closing = cards.iter().filter(|c| c.status == "CLOSING").count();
    let available = cards.iter().filter(|c| c.status == "AVAILABLE").count();
    let open_revenue: f64 = cards.iter().filter_map(|c| c.total).sum();
    let pct = if total > 0 { ((occupied + closing) as f64 / total as f64 * 100.0).round() as i64 } else { 0 };
    store.set_occupancy(format!("{} / {}", occupied + closing, total).into());
    store.set_occupancy_pct(format!("({pct}%)").into());
    store.set_free_count(available as i32);
    store.set_occupied_count(occupied as i32);
    store.set_closing_count(closing as i32);
    store.set_total_count(total as i32);
    store.set_open_revenue(brl(open_revenue).into());

    let mut sections: Vec<SharedString> = Vec::new();
    for c in cards {
        if !c.section.is_empty() && !sections.iter().any(|x| x.as_str() == c.section) {
            sections.push(c.section.clone().into());
        }
    }
    store.set_sections(model(sections));

    let section = store.get_section_filter();
    let status = store.get_status_filter();
    let query = store.get_search().trim().to_lowercase();
    let visible: Vec<MesaCard> = cards
        .iter()
        .filter(|c| section == "ALL" || c.section == section.as_str())
        .filter(|c| status == "ALL" || c.status == status.as_str())
        .filter(|c| {
            query.is_empty()
                || c.number.to_string().contains(&query)
                || c.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&query))
                || c.customer_name.as_deref().is_some_and(|n| n.to_lowercase().contains(&query))
        })
        .map(card_view)
        .collect();
    store.set_cards(model(visible));
}

/// Recarrega as mesas do banco (e o detalhe aberto, se houver).
pub(crate) fn refresh(ctx: &AppCtx) {
    if let Some(ui) = ctx.ui() {
        ui.global::<MesasStore>().set_loading(true);
    }
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { tables::list_cards(&db).await }, move |ui, result| {
        ui.global::<MesasStore>().set_loading(false);
        match result {
            Ok(cards) => {
                with_state(|s| {
                    s.cards = cards;
                    render(ui, s);
                });
                if ui.global::<MesasModalStore>().get_modal() == MesasModal::Details {
                    load_details(&next);
                }
            }
            Err(e) => next.status(format!("Erro ao carregar as mesas: {e}"), true),
        }
    });
}

fn modal(ui: &AppWindow, which: MesasModal) {
    let m = ui.global::<MesasModalStore>();
    m.set_error(SharedString::new());
    m.set_busy(false);
    m.set_modal(which);
}

pub(crate) fn close_modal(ui: &AppWindow) {
    modal(ui, MesasModal::None);
}

fn modal_error(ui: &AppWindow, message: impl Into<SharedString>) {
    let m = ui.global::<MesasModalStore>();
    m.set_busy(false);
    m.set_error(message.into());
}

pub(crate) fn card(id: &str) -> Option<TableCard> {
    with_state(|s| s.cards.iter().find(|c| c.id == id).cloned())
}

// ---------- Detalhe da mesa ----------

fn modifiers_label(json: Option<&str>) -> String {
    json.and_then(|m| serde_json::from_str::<Vec<ModifierSnapshot>>(m).ok())
        .map(|list| {
            list.iter()
                .map(|m| if m.price > 0.0 { format!("{} (+{})", m.name, brl(m.price)) } else { m.name.clone() })
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default()
}

fn render_details(ui: &AppWindow, card: &TableCard, detail: &OrderDetail) {
    let m = ui.global::<MesasModalStore>();
    let h = &detail.head;
    m.set_d_title(table_title(card).into());
    m.set_d_order(format!("CMD #{}", h.order_number).into());
    m.set_d_closing(card.status == "CLOSING");
    m.set_d_customer(
        format!(
            "{} ({}p)",
            card.customer_name.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "Cliente não identificado".into()),
            card.customer_count
        )
        .into(),
    );
    m.set_d_waiter(h.waiter_name.clone().unwrap_or_else(|| "Garçom".into()).into());
    m.set_d_elapsed(elapsed_long(card).into());
    let lines: Vec<DetailLine> = detail
        .items
        .iter()
        .map(|l| DetailLine {
            id: l.id.clone().into(),
            quantity: format!("{}x", l.quantity).into(),
            name: if l.unit_type.as_deref() == Some("BOX") { format!("{} (caixa)", l.product_name) } else { l.product_name.clone() }
                .into(),
            kds: if l.kds_station == "NONE" { SharedString::new() } else { l.kds_status.clone().into() },
            modifiers: modifiers_label(l.modifiers.as_deref()).into(),
            promotion: l.promotion_name.clone().unwrap_or_default().into(),
            notes: l.notes.clone().unwrap_or_default().into(),
            unit_info: format!(
                "{} cada • {}",
                brl(l.unit_price),
                match l.kds_station.as_str() {
                    "BAR" => "Bar",
                    "KITCHEN" => "Cozinha",
                    _ => "Sem preparo",
                }
            )
            .into(),
            total: brl(l.total_price).into(),
        })
        .collect();
    m.set_d_lines(model(lines));
    m.set_d_total(brl(h.total).into());
    m.set_d_fee(h.is_service_fee_active);
    m.set_d_haver(haver_label(h.total, h.paid_amount).into());
}

/// Carrega a comanda da mesa em foco e mostra (ou atualiza) o detalhe.
pub(crate) fn load_details(ctx: &AppCtx) {
    let Some(table_id) = with_state(|s| s.table_id.clone()) else { return };
    let Some(card) = card(&table_id) else { return };
    let Some(order_id) = card.order_id.clone() else {
        // A comanda foi fechada/transferida em outro aparelho
        if let Some(ui) = ctx.ui() {
            if ui.global::<MesasModalStore>().get_modal() == MesasModal::Details {
                close_modal(&ui);
            }
        }
        return;
    };
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { orders::detail(&db, &order_id).await }, move |ui, result| match result {
        Ok(detail) => {
            render_details(ui, &card, &detail);
            with_state(|s| s.detail = Some(detail));
            if ui.global::<MesasModalStore>().get_modal() == MesasModal::None {
                modal(ui, MesasModal::Details);
            }
        }
        Err(e) => next.status(format!("Erro ao abrir a comanda: {e}"), true),
    });
}

fn focus_table(id: &str) {
    with_state(|s| {
        s.table_id = Some(id.to_string());
        s.detail = None;
    });
}

// ---------- Abrir mesa ----------

fn show_open_table(ctx: &AppCtx, table_id: &str) {
    let (Some(ui), Some(card)) = (ctx.ui(), card(table_id)) else { return };
    focus_table(table_id);
    let m = ui.global::<MesasModalStore>();
    m.set_o_title(format!("Abrir {}", table_title(&card)).into());
    m.set_o_section(card.section.clone().into());
    m.set_o_people(card.capacity.clamp(1, 10) as i32);
    m.set_o_customer(SharedString::new());
    m.set_o_custom_waiter(false);
    m.set_o_waiter_name("Garçom".into());
    m.set_o_waiter_id(SharedString::new());
    modal(&ui, MesasModal::OpenTable);

    let db = ctx.db.clone();
    ctx.spawn(async move { waiters::list(&db, false).await }, |ui, result| {
        let m = ui.global::<MesasModalStore>();
        let list = result.unwrap_or_default();
        if let Some(first) = list.first() {
            m.set_o_waiter_id(first.id.clone().into());
            m.set_o_waiter_name(first.name.clone().into());
        }
        let choices: Vec<Choice> = list
            .into_iter()
            .map(|w| Choice { id: w.id.into(), label: w.name.into(), detail: SharedString::new(), selected: false })
            .collect();
        m.set_o_waiters(model(choices));
    });
}

fn confirm_open_table(ctx: &AppCtx) {
    let (Some(ui), Some(table_id)) = (ctx.ui(), with_state(|s| s.table_id.clone())) else { return };
    let m = ui.global::<MesasModalStore>();
    let custom = m.get_o_custom_waiter() || m.get_o_waiters().row_count() == 0;
    let input = OpenTable {
        customer_name: Some(m.get_o_customer().to_string()),
        waiter_id: (!custom).then(|| m.get_o_waiter_id().to_string()).filter(|id| !id.is_empty()),
        waiter_name: Some(m.get_o_waiter_name().trim().to_string()).filter(|n| !n.is_empty()),
        customer_count: i64::from(m.get_o_people()),
    };
    m.set_busy(true);
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { orders::open_table(&db, &table_id, input).await }, move |ui, result| match result {
        Ok((_, events)) => {
            next.emit(&events);
            close_modal(ui);
            refresh(&next);
        }
        Err(e) => modal_error(ui, e.to_string()),
    });
}

// ---------- Transferir / Juntar ----------

fn show_table_choice(ctx: &AppCtx, merge: bool) {
    let Some(ui) = ctx.ui() else { return };
    let Some(current) = with_state(|s| s.table_id.clone()) else { return };
    let Some(source) = card(&current) else { return };
    let choices: Vec<Choice> = with_state(|s| {
        s.cards
            .iter()
            .filter(|c| c.id != current)
            .filter(|c| if merge { c.order_id.is_some() } else { c.status == "AVAILABLE" && c.order_id.is_none() })
            .map(|c| Choice {
                id: c.id.clone().into(),
                label: table_title(c).into(),
                detail: if merge {
                    format!("{} · {}", c.customer_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| "Cliente".into()), brl(c.total.unwrap_or(0.0)))
                } else {
                    format!("{} lugares", c.capacity)
                }
                .into(),
                selected: false,
            })
            .collect()
    });
    let m = ui.global::<MesasModalStore>();
    m.set_t_choices(model(choices));
    m.set_t_selected(SharedString::new());
    if merge {
        m.set_t_title(format!("Juntar com {}", table_title(&source)).into());
        m.set_t_hint(
            "Escolha a mesa que vai entrar nesta comanda. Os itens e o que já foi pago (haver) vêm junto, e a outra mesa fica livre."
                .into(),
        );
        m.set_t_empty("Não há outras mesas ocupadas disponíveis para unir neste momento.".into());
        m.set_t_button("Confirmar Junção".into());
        modal(&ui, MesasModal::Merge);
    } else {
        m.set_t_title(format!("Transferir {}", table_title(&source)).into());
        m.set_t_hint("Escolha a mesa livre para onde a comanda inteira vai.".into());
        m.set_t_empty("Não há mesas livres no salão no momento para realizar a transferência.".into());
        m.set_t_button("Confirmar Transferência".into());
        modal(&ui, MesasModal::Transfer);
    }
}

fn confirm_table_choice(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    let merge = m.get_modal() == MesasModal::Merge;
    let target = m.get_t_selected().to_string();
    let Some(current) = with_state(|s| s.table_id.clone()) else { return };
    if target.is_empty() {
        return;
    }
    m.set_busy(true);
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(
        async move {
            if merge {
                orders::merge_tables(&db, &current, &target).await
            } else {
                orders::transfer_table(&db, &current, &target).await
            }
        },
        move |ui, result| match result {
            Ok((message, events)) => {
                next.emit(&events);
                close_modal(ui);
                next.status(message, false);
                refresh(&next);
            }
            Err(e) => modal_error(ui, e.to_string()),
        },
    );
}

// ---------- Confirmações (perda, cancelar item) ----------

fn ask_confirm(ui: &AppWindow, confirm: Confirm, title: &str, message: &str, button: &str) {
    with_state(|s| s.confirm = Some(confirm));
    let m = ui.global::<MesasModalStore>();
    m.set_q_title(title.into());
    m.set_q_message(message.into());
    m.set_q_button(button.into());
    m.set_q_danger(true);
    modal(ui, MesasModal::Confirm);
}

fn run_confirm(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let Some(confirm) = with_state(|s| s.confirm.clone()) else { return };
    ui.global::<MesasModalStore>().set_busy(true);
    let db = ctx.db.clone();
    let next = ctx.clone();
    let user = ctx.user_name();
    match confirm {
        Confirm::Loss(order_id) => ctx.spawn(
            async move { order_history::mark_as_loss(&db, &order_id, user.as_deref()).await },
            move |ui, result| match result {
                Ok((lost, events)) => {
                    next.emit(&events);
                    close_modal(ui);
                    next.status(format!("Baixa de perda registrada: {} na auditoria.", brl(lost)), false);
                    refresh(&next);
                }
                Err(e) => modal_error(ui, e.to_string()),
            },
        ),
        Confirm::RemoveItem { order_id, item_id } => ctx.spawn(
            async move { orders::remove_item(&db, &order_id, &item_id, user.as_deref()).await },
            move |ui, result| match result {
                Ok(events) => {
                    next.emit(&events);
                    modal(ui, MesasModal::Details);
                    refresh(&next);
                }
                Err(e) => modal_error(ui, e.to_string()),
            },
        ),
    }
}

// ---------- Nova mesa ----------

fn confirm_new_table(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    let Ok(number) = m.get_n_number().trim().parse::<i64>() else {
        modal_error(&ui, "Informe um número de mesa válido");
        return;
    };
    let input = TableInput {
        number,
        name: Some(m.get_n_name().to_string()),
        capacity: i64::from(m.get_n_capacity()),
        section: Some(m.get_n_section().to_string()),
    };
    m.set_busy(true);
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { table_admin::create_table(&db, input).await }, move |ui, result| match result {
        Ok((_, events)) => {
            next.emit(&events);
            close_modal(ui);
            next.status(format!("Mesa {number} criada."), false);
            refresh(&next);
        }
        Err(e) => modal_error(ui, e.to_string()),
    });
}

// ---------- Impressão ----------

fn print_bill(ctx: &AppCtx) {
    let Some(order_id) = with_state(|s| s.detail.as_ref().map(|d| d.head.id.clone())) else { return };
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { crate::print::print_order_receipt(&db, &order_id).await }, move |_, result| match result {
        Ok(message) => next.status(message, false),
        Err(e) => next.status(format!("Não foi possível imprimir: {e}"), true),
    });
}

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<MesasStore>();
    let m = ui.global::<MesasModalStore>();

    store.on_filters_changed({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                with_state(|s| render(&ui, s));
            }
        }
    });
    store.on_refresh({
        let ctx = ctx.clone();
        move || refresh(&ctx)
    });
    store.on_card_open({
        let ctx = ctx.clone();
        move |id| show_open_table(&ctx, &id)
    });
    store.on_card_select({
        let ctx = ctx.clone();
        move |id| {
            focus_table(&id);
            load_details(&ctx);
        }
    });
    store.on_card_order({
        let ctx = ctx.clone();
        move |id| {
            focus_table(&id);
            launch::open(&ctx);
        }
    });
    store.on_card_pay({
        let ctx = ctx.clone();
        move |id| {
            focus_table(&id);
            checkout::open(&ctx);
        }
    });
    store.on_open_new_table({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let m = ui.global::<MesasModalStore>();
            m.set_n_number(SharedString::new());
            m.set_n_name(SharedString::new());
            m.set_n_capacity(4);
            m.set_n_section("Salão Principal".into());
            modal(&ui, MesasModal::NewTable);
        }
    });
    store.on_open_history({
        let ctx = ctx.clone();
        move || history::open(&ctx)
    });
    store.on_open_order_history({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                ui.set_page(Page::Tables);
            }
            history::open(&ctx);
        }
    });
    store.on_open_waiters({
        let ctx = ctx.clone();
        move || equipe::open_waiters(&ctx)
    });
    store.on_open_staff({
        let ctx = ctx.clone();
        move || equipe::open_staff(&ctx)
    });
    store.on_open_connect_mobile({
        let ctx = ctx.clone();
        move || equipe::open_connect(&ctx)
    });
    store.on_open_manage({
        let ctx = ctx.clone();
        move || manage::open(&ctx)
    });

    // Detalhe
    m.on_d_add_order({
        let ctx = ctx.clone();
        move || launch::open(&ctx)
    });
    m.on_d_checkout({
        let ctx = ctx.clone();
        move || checkout::open(&ctx)
    });
    m.on_d_transfer({
        let ctx = ctx.clone();
        move || show_table_choice(&ctx, false)
    });
    m.on_d_merge({
        let ctx = ctx.clone();
        move || show_table_choice(&ctx, true)
    });
    m.on_d_closing_toggle({
        let ctx = ctx.clone();
        move || {
            let Some(table_id) = with_state(|s| s.table_id.clone()) else { return };
            let closing = card(&table_id).is_some_and(|c| c.status == "CLOSING");
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(
                async move {
                    if closing { orders::reopen(&db, &table_id).await } else { orders::request_closing(&db, &table_id).await }
                },
                move |_, result| match result {
                    Ok(events) => {
                        next.emit(&events);
                        refresh(&next);
                    }
                    Err(e) => next.status(e.to_string(), true),
                },
            );
        }
    });
    m.on_d_loss({
        let ctx = ctx.clone();
        move || {
            let (Some(ui), Some(order_id)) = (ctx.ui(), with_state(|s| s.detail.as_ref().map(|d| d.head.id.clone()))) else {
                return;
            };
            ask_confirm(
                &ui,
                Confirm::Loss(order_id),
                "Perda (Calote)",
                "ALERTA: Tem certeza que deseja dar baixa nesta mesa como PERDA/CALOTE? Isso não gera receita, mas debita o estoque e registra o prejuízo na Auditoria Cega.",
                "Registrar Perda",
            );
        }
    });
    m.on_d_remove({
        let ctx = ctx.clone();
        move |item_id| {
            let (Some(ui), Some(order_id)) = (ctx.ui(), with_state(|s| s.detail.as_ref().map(|d| d.head.id.clone()))) else {
                return;
            };
            ask_confirm(
                &ui,
                Confirm::RemoveItem { order_id, item_id: item_id.to_string() },
                "Cancelar item",
                "Deseja realmente cancelar este item do pedido? O estoque será reposto.",
                "Cancelar Item",
            );
        }
    });
    m.on_d_print({
        let ctx = ctx.clone();
        move || print_bill(&ctx)
    });

    m.on_o_confirm({
        let ctx = ctx.clone();
        move || confirm_open_table(&ctx)
    });
    m.on_t_confirm({
        let ctx = ctx.clone();
        move || confirm_table_choice(&ctx)
    });
    m.on_n_confirm({
        let ctx = ctx.clone();
        move || confirm_new_table(&ctx)
    });
    m.on_q_confirm({
        let ctx = ctx.clone();
        move || run_confirm(&ctx)
    });
    m.on_close({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let current = ui.global::<MesasModalStore>().get_modal();
            // Cancelar uma confirmação de item volta para o detalhe da mesa
            let back_to_details = current == MesasModal::Confirm
                && with_state(|s| matches!(s.confirm, Some(Confirm::RemoveItem { .. })));
            with_state(|s| s.confirm = None);
            if back_to_details || matches!(current, MesasModal::Transfer | MesasModal::Merge) && with_state(|s| s.detail.is_some()) {
                modal(&ui, MesasModal::Details);
            } else {
                close_modal(&ui);
            }
        }
    });

    // Atualização: ao entrar na página, a cada 5 s e na hora em que os celulares gravam algo
    let timer = slint::Timer::default();
    let last = std::cell::Cell::new(None::<std::time::Instant>);
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), {
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            if ui.get_page() != Page::Tables {
                last.set(None);
                return;
            }
            let due = last.get().is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(5));
            let idle = !matches!(
                ui.global::<MesasModalStore>().get_modal(),
                MesasModal::AddOrder | MesasModal::Modifiers | MesasModal::Checkout | MesasModal::OpenTable | MesasModal::NewTable
            );
            if due && idle {
                last.set(Some(std::time::Instant::now()));
                refresh(&ctx);
            }
        }
    });
    TIMER.with(|t| *t.borrow_mut() = Some(timer));
    if let Some(server) = &ctx.server {
        let mut events = server.subscribe();
        let ctx2 = ctx.clone();
        ctx.rt.spawn(async move {
            loop {
                let name = match events.recv().await {
                    Ok(ev) => ev.name,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if name.starts_with("table:") || name.starts_with("order:") || name.starts_with("kds:") {
                    let ctx3 = ctx2.clone();
                    let _ = ctx2.ui.upgrade_in_event_loop(move |ui| {
                        if ui.get_page() == Page::Tables {
                            refresh(&ctx3);
                        }
                    });
                }
            }
        });
    }

    launch::bind(ctx);
    checkout::bind(ctx);
    equipe::bind(ctx);
    history::bind(ctx);
    manage::bind(ctx);
}

/// Preenche a página e os modais com dados já carregados, sem tokio — usado pelo gerador de prints
/// (`examples/screenshot.rs`) para conferir o visual.
pub mod preview {
    use super::*;
    use barerp_db::catalog::{Category, SaleProduct};
    use barerp_core::pricing::Promotion;

    pub fn map(ui: &AppWindow, cards: Vec<TableCard>) {
        with_state(|s| {
            s.cards = cards;
            render(ui, s);
        });
    }

    pub fn details(ui: &AppWindow, table_id: &str, detail: OrderDetail) {
        if let Some(card) = card(table_id) {
            focus_table(table_id);
            render_details(ui, &card, &detail);
            with_state(|s| s.detail = Some(detail));
            modal(ui, MesasModal::Details);
        }
    }

    pub fn launch(ui: &AppWindow, table_id: &str, products: Vec<SaleProduct>, categories: Vec<Category>, promotions: Vec<Promotion>) {
        focus_table(table_id);
        launch::preview(ui, table_id, products, categories, promotions);
    }

    pub fn manage(ui: &AppWindow) {
        manage::open_preview(ui);
    }

    pub fn waiters(ui: &AppWindow, list: Vec<barerp_db::waiters::Waiter>) {
        equipe::preview_waiters(ui, list);
    }

    pub fn staff(ui: &AppWindow, list: Vec<barerp_db::staff::StaffMember>) {
        equipe::preview_staff(ui, list);
    }

    pub fn connect(ui: &AppWindow) {
        equipe::preview_connect(ui);
    }

    pub fn checkout(ui: &AppWindow, table_id: &str, detail: OrderDetail, mode: &str, method: &str, amount: &str) {
        focus_table(table_id);
        checkout::preview(ui, detail, mode, method, amount);
    }
}

/// Fecha qualquer modal da área (gerador de prints).
pub fn close_preview(ui: &AppWindow) {
    close_modal(ui);
}
