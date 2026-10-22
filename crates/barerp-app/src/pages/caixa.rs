//! Página "caixa": CashView (turno, sangria/suprimento, fechamento cego, comissões) e QuickSaleModal (venda
//! rápida de balcão com NFC-e em segundo plano). Ver `paridade/caixa.md` para a lista item por item.

use std::cell::RefCell;

use chrono::Local;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use barerp_core::fiscal_doc::{cpf_cnpj_error, NFCE_LIMITE_SEM_IDENTIFICACAO};
use barerp_core::money::brl;
use barerp_core::pricing::{apply_best_promotion, resolve_modifiers, ModifierGroup, Promotion};
use barerp_db::caixa::{self as caixa_db, PdvProduct, TurnPayment};
use barerp_db::cash::{self, CloseShiftInput, CurrentShiftState, OpenShiftInput, TransactionInput, WaiterCommission};
use barerp_db::catalog::{self, Category};
use barerp_db::orders::{self, NewLine, OrderDetail, QuickSaleInput, QuickSalePayment};

use crate::ctx::AppCtx;
use crate::pages::fiscal::{self as fiscal_page, ItemVendaNfce, ResultadoNfce, VendaNfce};
use crate::{AppWindow, CaixaStore, CxCartLine, CxCategory, CxGroup, CxNfce, CxOption, CxPayment, CxProduct, CxWaiter};

/// Linha do carrinho do balcão. Mesmo produto + mesma unidade + mesmas opções somam na mesma linha.
#[derive(Clone)]
struct CartItem {
    key: String,
    product_id: String,
    is_box: bool,
    /// (id, nome, preço) na ordem do cadastro
    options: Vec<(String, String, f64)>,
    qty: i64,
    notes: Option<String>,
}

struct Picker {
    product_id: String,
    /// Quantidade pedida na busca ("3*código")
    qty: i64,
    groups: Vec<(String, ModifierGroup)>,
    selected: Vec<Vec<bool>>,
    show_errors: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum NfceKind {
    Emitting,
    Authorized,
    Processing,
    Error,
}

/// Situação da NFC-e de uma venda (a emissão roda depois da venda gravada e não trava o próximo atendimento).
struct NfceEntry {
    order_number: i64,
    kind: NfceKind,
    message: String,
    referencia: Option<String>,
    /// Para "Tentar de novo" (a referência é fixa por venda, então não duplica a nota)
    venda: VendaNfce,
}

#[derive(Default)]
struct State {
    loaded: bool,
    is_open: bool,
    expected: f64,
    pending_shortcut: Option<String>,

    quick_loaded: bool,
    products: Vec<PdvProduct>,
    categories: Vec<Category>,
    promotions: Vec<Promotion>,
    /// Mais vendidos (ids, do mais vendido para o menos), para a aba "Mais vendidos"
    top: Vec<String>,
    cart: Vec<CartItem>,
    picker: Option<Picker>,
    nfce: Vec<NfceEntry>,
    submitting: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// "Mais vendidos": conta os últimos itens lançados e mostra os primeiros desta lista.
const TOP_RECENT_ITEMS: i64 = 5000;
const TOP_LIMIT: i64 = 15;

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// Valor digitado com vírgula ou ponto (`parseFloat` do original).
fn parse_amount(text: &str) -> Option<f64> {
    let t = text.trim().replace(',', ".");
    let end = t.char_indices().find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && *c == '-'))).map_or(t.len(), |(i, _)| i);
    t[..end].parse::<f64>().ok().filter(|v| v.is_finite())
}

fn method_label(method: &str) -> &str {
    match method {
        "CASH" => "Dinheiro",
        "CREDIT_CARD" => "Crédito",
        "DEBIT_CARD" => "Débito",
        "PIX" => "PIX",
        "VOUCHER" => "Voucher",
        "CREDIT_TAB" => "Fiado",
        other => other,
    }
}

fn alert(ui: &AppWindow, text: impl Into<SharedString>) {
    ui.global::<CaixaStore>().set_alert_text(text.into());
}

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();

    let c = ctx.clone();
    store.on_refresh(move || refresh(&c));
    let c = ctx.clone();
    // O histórico de comandas (OrderHistoryModal) é da área Mesas
    store.on_open_history(move || {
        if let Some(ui) = c.ui() {
            ui.global::<crate::MesasStore>().invoke_open_order_history();
        }
    });
    let c = ctx.clone();
    store.on_open_shift(move || open_shift(&c));
    let c = ctx.clone();
    store.on_reopen_last(move || {
        if let Some(ui) = c.ui() {
            let store = ui.global::<CaixaStore>();
            store.set_confirm_action("reopen".into());
            store.set_confirm_text("Tem certeza que deseja reabrir o último turno de caixa fechado para corrigir valores?".into());
        }
    });
    let c = ctx.clone();
    store.on_confirm_yes(move || confirm_yes(&c));
    let c = ctx.clone();
    store.on_ask_tx(move |withdrawal| {
        if let Some(ui) = c.ui() {
            let store = ui.global::<CaixaStore>();
            store.set_tx_withdrawal(withdrawal);
            store.set_tx_visible(true);
        }
    });
    let c = ctx.clone();
    store.on_tx_confirm(move || tx_confirm(&c));
    let c = ctx.clone();
    store.on_close_start(move || {
        if let Some(ui) = c.ui() {
            let store = ui.global::<CaixaStore>();
            store.set_close_step(1);
            store.set_close_visible(true);
        }
    });
    let c = ctx.clone();
    store.on_close_verify(move || close_verify(&c));
    let c = ctx.clone();
    store.on_close_confirm(move || close_confirm(&c));
    let c = ctx.clone();
    store.on_close_finish(move || {
        if let Some(ui) = c.ui() {
            let store = ui.global::<CaixaStore>();
            store.set_close_visible(false);
            store.set_close_step(1);
        }
    });
    let c = ctx.clone();
    store.on_shortcut(move |action| shortcut(&c, action.as_str()));

    // Venda rápida
    let c = ctx.clone();
    store.on_quick_open(move || quick_open(&c));
    let c = ctx.clone();
    store.on_quick_close(move || quick_close(&c));
    let c = ctx.clone();
    store.on_search_edited(move |_| render_quick(&c));
    let c = ctx.clone();
    store.on_search_enter(move || search_enter(&c));
    let c = ctx.clone();
    store.on_pick_category(move |id| {
        if let Some(ui) = c.ui() {
            ui.global::<CaixaStore>().set_category(id);
        }
        render_quick(&c);
    });
    let c = ctx.clone();
    store.on_add_product(move |id, is_box| add_product(&c, id.as_str(), is_box, false, 1));
    let c = ctx.clone();
    store.on_keypad(move |key| keypad(&c, key.as_str()));
    let c = ctx.clone();
    store.on_cart_qty(move |key, delta| {
        with_state(|s| {
            if let Some(item) = s.cart.iter_mut().find(|i| i.key == key.as_str()) {
                item.qty += delta as i64;
            }
            s.cart.retain(|i| i.qty > 0);
        });
        render_quick(&c);
    });
    let c = ctx.clone();
    store.on_cart_remove(move |key| {
        with_state(|s| s.cart.retain(|i| i.key != key.as_str()));
        render_quick(&c);
    });
    let c = ctx.clone();
    store.on_cart_clear(move || {
        with_state(|s| s.cart.clear());
        render_quick(&c);
    });
    let c = ctx.clone();
    store.on_recompute(move || render_quick(&c));
    let c = ctx.clone();
    store.on_finish_sale(move || finish_sale(&c));
    let c = ctx.clone();
    store.on_dismiss_success(move || {
        if let Some(ui) = c.ui() {
            let store = ui.global::<CaixaStore>();
            store.set_success_text(SharedString::new());
            store.set_success_change(SharedString::new());
        }
    });
    let c = ctx.clone();
    store.on_nfce_reprint(move |n| {
        let referencia = with_state(|s| s.nfce.iter().find(|e| e.order_number == n as i64).and_then(|e| e.referencia.clone()));
        if let Some(r) = referencia {
            fiscal_page::imprimir_danfe_nfce(&c, &r);
        }
    });
    let c = ctx.clone();
    store.on_nfce_retry(move |n| {
        let venda = with_state(|s| s.nfce.iter().find(|e| e.order_number == n as i64).map(|e| e.venda.clone()));
        if let Some(v) = venda {
            run_nfce(&c, v, None);
        }
    });
    let c = ctx.clone();
    store.on_nfce_dismiss(move |n| {
        with_state(|s| s.nfce.retain(|e| e.order_number != n as i64 || e.kind == NfceKind::Emitting));
        render_nfce(&c);
    });
    let c = ctx.clone();
    store.on_picker_toggle(move |g, o| picker_toggle(&c, g as usize, o as usize));
    let c = ctx.clone();
    store.on_picker_confirm(move || picker_confirm(&c));
    let c = ctx.clone();
    store.on_picker_cancel(move || {
        with_state(|s| s.picker = None);
        if let Some(ui) = c.ui() {
            let store = ui.global::<CaixaStore>();
            store.set_picker_visible(false);
            store.set_focus_search_tick(store.get_focus_search_tick() + 1);
        }
    });

    if let Some(user) = ctx.user_name() {
        store.set_open_by(user.clone().into());
        store.set_close_by(user.into());
    }
    refresh(ctx);
    listen_cash_updates(ctx);
}

/// Outro aparelho (ou outra tela do app) mexeu no caixa: atualiza o resumo na hora.
fn listen_cash_updates(ctx: &AppCtx) {
    let Some(server) = ctx.server.clone() else { return };
    let mut rx = server.subscribe();
    let c = ctx.clone();
    ctx.rt.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.name == "cash:updated" => {
                    let c2 = c.clone();
                    let _ = slint::invoke_from_event_loop(move || refresh(&c2));
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

// ======================================================================
// Turno
// ======================================================================

fn refresh(ctx: &AppCtx) {
    if let Some(ui) = ctx.ui() {
        ui.global::<CaixaStore>().set_loading(true);
    }
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move {
            let state = cash::current_shift(&db).await?;
            let payments = match &state.shift {
                Some(shift) if state.is_open => caixa_db::turn_payments(&db, &shift.id).await?,
                _ => Vec::new(),
            };
            Ok::<_, barerp_db::DbError>((state, payments))
        },
        move |ui, result| {
            let store = ui.global::<CaixaStore>();
            store.set_loading(false);
            match result {
                Ok((state, payments)) => {
                    render_shift(ui, &state, &payments);
                    let pending = with_state(|s| {
                        s.loaded = true;
                        s.is_open = state.is_open;
                        s.expected = state.summary.as_ref().map_or(0.0, |sm| sm.expected_cash_in_drawer);
                        s.pending_shortcut.take()
                    });
                    if let Some(action) = pending {
                        shortcut(&c, &action);
                    }
                }
                Err(e) => c.status(format!("Erro ao carregar caixa: {e}"), true),
            }
        },
    );
}

fn waiter_items(list: &[WaiterCommission], long: bool) -> Vec<CxWaiter> {
    list.iter()
        .map(|w| {
            let n = w.orders_count;
            let orders = match (long, n == 1) {
                (true, true) => format!("{n} comanda atendida"),
                (true, false) => format!("{n} comandas atendidas"),
                (false, true) => format!("{n} comanda"),
                (false, false) => format!("{n} comandas"),
            };
            CxWaiter {
                initials: w.waiter_name.chars().take(2).collect::<String>().to_uppercase().into(),
                name: w.waiter_name.clone().into(),
                orders: orders.into(),
                sales: brl(w.total_sales).into(),
                fee: brl(w.service_fee_total).into(),
            }
        })
        .collect()
}

fn render_shift(ui: &AppWindow, state: &CurrentShiftState, payments: &[TurnPayment]) {
    let store = ui.global::<CaixaStore>();
    store.set_is_open(state.is_open);
    let Some(summary) = &state.summary else {
        store.set_waiters(model(Vec::new()));
        store.set_payments(model(Vec::new()));
        return;
    };
    let by = |m: &str| brl(summary.payments_by_method.get(m).copied().unwrap_or(0.0));
    store.set_opened_by(state.shift.as_ref().map_or("Caixa".to_string(), |s| s.opened_by.clone()).into());
    store.set_initial(brl(summary.initial_balance).into());
    store.set_total_sales(brl(summary.total_sales).into());
    store.set_supplies(brl(summary.total_supplies).into());
    store.set_withdrawals(brl(summary.total_withdrawals).into());
    store.set_expected(brl(summary.expected_cash_in_drawer).into());
    store.set_pix(by("PIX").into());
    store.set_credit(by("CREDIT_CARD").into());
    store.set_debit(by("DEBIT_CARD").into());
    store.set_cash(by("CASH").into());
    store.set_voucher(by("VOUCHER").into());
    store.set_fee_total(brl(summary.total_service_fees_shift).into());
    store.set_waiters(model(waiter_items(&summary.waiter_commissions, true)));
    store.set_payments(model(
        payments
            .iter()
            .map(|p| CxPayment {
                place: p.place.clone().unwrap_or_else(|| "Balcão".into()).into(),
                method: method_label(&p.method).into(),
                notes: p.notes.clone().unwrap_or_default().into(),
                amount: brl(p.amount).into(),
            })
            .collect(),
    ));
}

fn open_shift(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    let Some(balance) = parse_amount(&store.get_open_balance()).filter(|v| *v >= 0.0) else {
        alert(&ui, "Informe um fundo de troco inicial válido");
        return;
    };
    let by = store.get_open_by().trim().to_string();
    if by.is_empty() {
        alert(&ui, "Informe o operador / responsável pelo caixa");
        return;
    }
    let notes = store.get_open_notes().trim().to_string();
    let input = OpenShiftInput { initial_balance: balance, opened_by: Some(by), notes: (!notes.is_empty()).then_some(notes) };
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(async move { cash::open_shift(&db, input).await }, move |ui, result| match result {
        Ok((_, events)) => {
            c.emit(&events);
            ui.global::<CaixaStore>().set_open_notes(SharedString::new());
            c.status("Turno de caixa aberto com sucesso!", false);
            refresh(&c);
        }
        Err(e) => alert(ui, e.to_string()),
    });
}

fn confirm_yes(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    let action = store.get_confirm_action();
    store.set_confirm_text(SharedString::new());
    match action.as_str() {
        "reopen" => {
            let db = ctx.db.clone();
            let c = ctx.clone();
            ctx.spawn(async move { cash::reopen_last_shift(&db).await }, move |ui, result| match result {
                Ok((_, events)) => {
                    c.emit(&events);
                    let store = ui.global::<CaixaStore>();
                    store.set_close_visible(false);
                    store.set_close_step(1);
                    refresh(&c);
                    alert(ui, "Turno de caixa reaberto com sucesso! O caixa está aberto para correções.");
                }
                Err(e) => alert(ui, e.to_string()),
            });
        }
        "close-quick" => close_quick_now(ctx),
        _ => {}
    }
}

fn tx_confirm(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    let amount = parse_amount(&store.get_tx_amount()).filter(|v| *v > 0.0);
    let reason = store.get_tx_reason().trim().to_string();
    let (Some(amount), false) = (amount, reason.is_empty()) else {
        alert(&ui, "Informe o valor e o motivo da movimentação");
        return;
    };
    let tx_type = if store.get_tx_withdrawal() { cash::TX_WITHDRAWAL } else { cash::TX_SUPPLY };
    let input = TransactionInput { tx_type: tx_type.into(), amount, reason };
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(async move { cash::add_transaction(&db, input).await }, move |ui, result| match result {
        Ok((_, events)) => {
            c.emit(&events);
            let store = ui.global::<CaixaStore>();
            let msg = if store.get_tx_withdrawal() { "Sangria registrada com sucesso." } else { "Suprimento registrado com sucesso." };
            store.set_tx_visible(false);
            store.set_tx_amount(SharedString::new());
            store.set_tx_reason(SharedString::new());
            c.status(msg, false);
            refresh(&c);
        }
        Err(e) => alert(ui, e.to_string()),
    });
}

/// Diferença como no original: `+ R$ 1,50` (sobra), `R$ 0,00` (exato), `-R$ 2,00` (falta).
fn signed_brl(value: f64) -> String {
    if value > 0.0 {
        format!("+ {}", brl(value))
    } else {
        brl(value)
    }
}

fn close_verify(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    let Some(counted) = parse_amount(&store.get_close_count()).filter(|v| *v >= 0.0) else {
        alert(&ui, "Informe um valor válido em dinheiro contado na gaveta");
        return;
    };
    if store.get_close_by().trim().is_empty() {
        alert(&ui, "Informe quem conferiu o caixa");
        return;
    }
    let expected = with_state(|s| s.expected);
    let diff = (counted - expected) * 100.0;
    let diff = diff.round() / 100.0;
    store.set_check_expected(brl(expected).into());
    store.set_check_counted(brl(counted).into());
    store.set_check_diff(signed_brl(diff).into());
    store.set_check_sign(if diff.abs() < 0.005 { 0 } else if diff > 0.0 { 1 } else { -1 });
    store.set_close_step(2);
}

fn close_confirm(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    if store.get_close_submitting() {
        return;
    }
    let Some(counted) = parse_amount(&store.get_close_count()) else { return };
    let input = CloseShiftInput {
        final_cash_count: counted,
        closed_by: Some(store.get_close_by().trim().to_string()),
        notes: None,
    };
    store.set_close_submitting(true);
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(async move { cash::close_shift(&db, input).await }, move |ui, result| {
        let store = ui.global::<CaixaStore>();
        store.set_close_submitting(false);
        match result {
            Ok((_, report, events)) => {
                c.emit(&events);
                store.set_rep_expected(brl(report.expected_cash).into());
                store.set_rep_counted(brl(report.counted_cash).into());
                store.set_rep_diff(signed_brl(report.difference).into());
                store.set_rep_negative(report.difference < 0.0);
                store.set_rep_fee_total(brl(report.total_service_fees_shift).into());
                store.set_rep_waiters(model(waiter_items(&report.waiter_commissions, false)));
                store.set_close_count(SharedString::new());
                store.set_close_step(3);
                refresh(&c);
                // Backup logo depois de fechar o caixa, como o Node (POST /cash/close)
                crate::pages::retaguarda::backup_after_cash_close(&c);
            }
            Err(e) => alert(ui, e.to_string()),
        }
    });
}

/// Atalhos do app (F2, Ctrl+A, Ctrl+S, Ctrl+Shift+S, Ctrl+F): esperam o turno carregar e não abrem por cima
/// de outra janela. `exactCash` é o botão "Exato" do troco.
fn shortcut(ctx: &AppCtx, action: &str) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    if action == "exactCash" {
        let total = cart_totals(&store).2;
        store.set_tendered(format!("{total:.2}").into());
        render_quick(ctx);
        return;
    }
    let (loaded, is_open) = with_state(|s| (s.loaded, s.is_open));
    if !loaded {
        with_state(|s| s.pending_shortcut = Some(action.to_string()));
        return;
    }
    if store.get_quick_visible() || store.get_tx_visible() || store.get_close_visible() {
        return;
    }
    if action == "openShift" {
        if is_open {
            alert(&ui, "O caixa já está aberto.");
        } else {
            // Abrir exige conferir o fundo de troco: o cursor vai para o campo (Enter confirma)
            store.set_focus_open_tick(store.get_focus_open_tick() + 1);
        }
        return;
    }
    if !is_open {
        alert(&ui, "O caixa está fechado. Abra o turno antes de usar este atalho.");
        return;
    }
    match action {
        "quickSale" => quick_open(ctx),
        "withdrawal" => store.invoke_ask_tx(true),
        "supply" => store.invoke_ask_tx(false),
        "closeShift" => store.invoke_close_start(),
        _ => {}
    }
}

// ======================================================================
// Venda rápida de balcão
// ======================================================================

fn quick_open(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    if !with_state(|s| s.is_open) {
        alert(&ui, "O caixa está fechado. Abra o turno antes de usar este atalho.");
        return;
    }
    // Abre como o modal novo do original: carrinho vazio e NFC-e desligada
    with_state(|s| {
        s.cart.clear();
        s.picker = None;
        s.nfce.clear();
        s.submitting = false;
    });
    store.set_search(SharedString::new());
    // Abre nos mais vendidos (sem histórico de vendas, a lista mostra todos)
    store.set_category("TOP".into());
    store.set_discount(SharedString::new());
    store.set_customer(SharedString::new());
    store.set_tendered(SharedString::new());
    store.set_method("PIX".into());
    store.set_emit_nfce(false);
    store.set_nfce_doc(SharedString::new());
    store.set_print_receipt(true);
    store.set_success_text(SharedString::new());
    store.set_success_change(SharedString::new());
    store.set_picker_visible(false);
    store.set_quick_visible(true);
    render_nfce(ctx);
    load_catalog(ctx);
}

/// Cardápio, categorias e promoções do dia. Mostra o que já tem e atualiza em seguida (cadastro novo no PC).
fn load_catalog(ctx: &AppCtx) {
    let first = !with_state(|s| s.quick_loaded);
    if let Some(ui) = ctx.ui() {
        ui.global::<CaixaStore>().set_quick_loading(first);
    }
    render_quick(ctx);
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move {
            let products = caixa_db::pdv_products(&db).await?;
            let categories = catalog::categories(&db).await?;
            let promotions = catalog::promotions(&db).await?;
            let top = caixa_db::top_sellers(&db, TOP_RECENT_ITEMS, TOP_LIMIT).await?;
            Ok::<_, barerp_db::DbError>((products, categories, promotions, top))
        },
        move |ui, result| {
            let store = ui.global::<CaixaStore>();
            store.set_quick_loading(false);
            match result {
                Ok((products, categories, promotions, top)) => {
                    store.set_has_top(!top.is_empty());
                    with_state(|s| {
                        s.products = products;
                        s.categories = categories;
                        s.promotions = promotions;
                        s.top = top;
                        s.quick_loaded = true;
                    });
                    render_quick(&c);
                    store.set_focus_search_tick(store.get_focus_search_tick() + 1);
                }
                Err(e) => c.status(format!("Erro ao carregar cardápio: {e}"), true),
            }
        },
    );
}

fn quick_close(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    if with_state(|s| s.picker.is_some()) {
        ui.global::<CaixaStore>().invoke_picker_cancel();
        return;
    }
    if with_state(|s| s.nfce.iter().any(|e| e.kind == NfceKind::Emitting)) {
        let store = ui.global::<CaixaStore>();
        store.set_confirm_action("close-quick".into());
        store.set_confirm_text(
            "Ainda há NFC-e sendo emitida. Se fechar agora, um erro na emissão não vai aparecer aqui (confira depois em Reimpressão de NFC-e). Fechar mesmo assim?".into(),
        );
        return;
    }
    close_quick_now(ctx);
}

fn close_quick_now(ctx: &AppCtx) {
    if let Some(ui) = ctx.ui() {
        ui.global::<CaixaStore>().set_quick_visible(false);
    }
    refresh(ctx);
}

/// Preço da unidade agora (promoção do dia) e o nome da promoção.
fn unit_price(p: &PdvProduct, promos: &[Promotion]) -> (f64, Option<String>) {
    let (price, promo) = apply_best_promotion(p.price, &p.id, &p.category_id, promos, Local::now().naive_local());
    (price, promo.map(|pr| pr.name.clone()))
}

fn line_price(item: &CartItem, p: &PdvProduct, promos: &[Promotion]) -> f64 {
    if item.is_box {
        p.box_price.unwrap_or(0.0)
    } else {
        unit_price(p, promos).0 + item.options.iter().map(|o| o.2).sum::<f64>()
    }
}

/// (subtotal, desconto aplicado, total) do carrinho.
fn cart_totals(store: &CaixaStore) -> (f64, f64, f64) {
    let subtotal = with_state(|s| {
        s.cart
            .iter()
            .filter_map(|i| s.products.iter().find(|p| p.id == i.product_id).map(|p| line_price(i, p, &s.promotions) * i.qty as f64))
            .sum::<f64>()
    });
    let discount = parse_amount(&store.get_discount()).unwrap_or(0.0).max(0.0).min(subtotal);
    (subtotal, discount, (subtotal - discount).max(0.0))
}

fn matches_search(p: &PdvProduct, raw: &str) -> bool {
    if raw.is_empty() {
        return true;
    }
    let clean = raw.strip_prefix('#').unwrap_or(raw);
    let has = |v: &Option<String>, term: &str| !term.is_empty() && v.as_deref().is_some_and(|x| x.to_lowercase().contains(term));
    p.name.to_lowercase().contains(raw)
        || has(&p.code, clean)
        || has(&p.ean, clean)
        || has(&p.box_ean, clean)
        || p.brand.as_deref().is_some_and(|b| b.to_lowercase().contains(raw))
        || p.description.as_deref().is_some_and(|d| d.to_lowercase().contains(raw))
}

/// "3*7891000" → (3, "7891000"): quantidade antes do asterisco, como nos PDVs de supermercado.
fn split_qty(raw: &str) -> (i64, &str) {
    if let Some((n, rest)) = raw.split_once('*') {
        if let Ok(qty) = n.trim().parse::<i64>() {
            if (1..=999).contains(&qty) {
                return (qty, rest.trim());
            }
        }
    }
    (1, raw)
}

fn filtered_ids(store: &CaixaStore) -> Vec<String> {
    let category = store.get_category().to_string();
    let search = store.get_search().trim().to_lowercase();
    let (_, raw) = split_qty(&search);
    with_state(|s| {
        // Mais vendidos na ordem de venda; com busca digitada (ou sem histórico) vale o cardápio todo
        if category == "TOP" && raw.is_empty() && !s.top.is_empty() {
            return s.top.iter().filter(|id| s.products.iter().any(|p| &p.id == *id)).cloned().collect();
        }
        s.products
            .iter()
            .filter(|p| (category == "ALL" || category == "TOP" || p.category_id == category) && matches_search(p, raw))
            .map(|p| p.id.clone())
            .collect()
    })
}

fn render_quick(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    let ids = filtered_ids(&store);

    let (products, categories, all_label, cart, cart_count, submitting) = with_state(|s| {
        let qty = |pid: &str, is_box: bool| -> i32 {
            s.cart.iter().filter(|i| i.product_id == pid && i.is_box == is_box).map(|i| i.qty as i32).sum()
        };
        let products: Vec<CxProduct> = ids
            .iter()
            .filter_map(|id| s.products.iter().find(|p| &p.id == id))
            .map(|p| {
                let (price, promo) = unit_price(p, &s.promotions);
                let (units, boxes) = (qty(&p.id, false), qty(&p.id, true));
                let box_price = p.box_price.unwrap_or(0.0);
                CxProduct {
                    id: p.id.clone().into(),
                    name: p.name.clone().into(),
                    code: p.code.clone().unwrap_or_default().into(),
                    description: p.description.clone().unwrap_or_default().into(),
                    price: brl(price).into(),
                    full_price: brl(p.price).into(),
                    promo: promo.is_some(),
                    sells_box: p.sells_box(),
                    box_label: format!("Cx {}x", p.box_units()).into(),
                    box_price: brl(box_price).into(),
                    box_per_unit: format!("{}/un", brl(box_price / p.box_units() as f64)).into(),
                    track_stock: p.track_stock,
                    low_stock: p.low_stock(),
                    stock: p.stock.to_string().into(),
                    in_cart: units + boxes,
                    units,
                    boxes,
                }
            })
            .collect();
        let categories: Vec<CxCategory> =
            s.categories.iter().map(|c| CxCategory { id: c.id.clone().into(), label: c.name.clone().into() }).collect();
        let cart: Vec<CxCartLine> = s
            .cart
            .iter()
            .filter_map(|i| {
                let p = s.products.iter().find(|p| p.id == i.product_id)?;
                let price = line_price(i, p, &s.promotions);
                let promo = if i.is_box { None } else { unit_price(p, &s.promotions).1 };
                Some(CxCartLine {
                    key: i.key.clone().into(),
                    name: p.name.clone().into(),
                    is_box: i.is_box,
                    box_label: format!("Cx {}x", p.box_units()).into(),
                    unit_price: format!("{} {}", brl(price), if i.is_box { "cx" } else { "un" }).into(),
                    options: i.options.iter().map(|o| o.1.as_str()).collect::<Vec<_>>().join(" · ").into(),
                    promo: promo.unwrap_or_default().into(),
                    qty: i.qty as i32,
                    total: brl(price * i.qty as f64).into(),
                })
            })
            .collect();
        let count: i64 = s.cart.iter().map(|i| i.qty).sum();
        (products, categories, format!("Todos ({})", s.products.len()), cart, count, s.submitting)
    });

    store.set_products(model(products));
    store.set_categories(model(categories));
    store.set_all_label(all_label.into());
    store.set_cart(model(cart));
    store.set_cart_count(cart_count as i32);
    store.set_submitting(submitting);

    let (subtotal, discount, total) = cart_totals(&store);
    store.set_subtotal_label(brl(subtotal).into());
    store.set_has_discount(discount > 0.0);
    store.set_total_label(brl(total).into());
    store.set_total_placeholder(brl(total).into());
    let tendered = parse_amount(&store.get_tendered()).filter(|v| *v > 0.0).unwrap_or(total);
    let change = if store.get_method() == "CASH" && tendered > total { tendered - total } else { 0.0 };
    store.set_change_label(if change > 0.0 { brl(change).into() } else { SharedString::new() });

    let doc = store.get_nfce_doc();
    let has_digits = doc.chars().any(|c| c.is_ascii_digit());
    let doc_error = if has_digits { cpf_cnpj_error(&doc).unwrap_or_default() } else { "" };
    store.set_nfce_doc_error(doc_error.into());
}

fn render_nfce(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let items: Vec<CxNfce> = with_state(|s| {
        s.nfce
            .iter()
            .map(|e| {
                let (kind, label) = match e.kind {
                    NfceKind::Emitting => ("emitting", "sendo emitida..."),
                    NfceKind::Authorized => ("authorized", "autorizada"),
                    NfceKind::Processing => ("processing", "em processamento"),
                    NfceKind::Error => ("error", "não emitida"),
                };
                let msg = if e.message.is_empty() { String::new() } else { format!(": {}", e.message) };
                CxNfce {
                    order_number: e.order_number as i32,
                    kind: kind.into(),
                    text: format!("Venda #{} · NFC-e {label}{msg}", e.order_number).into(),
                    can_reprint: e.kind == NfceKind::Authorized && e.referencia.is_some(),
                    can_retry: e.kind == NfceKind::Error,
                }
            })
            .collect()
    });
    ui.global::<CaixaStore>().set_nfce_list(model(items));
}

fn cart_key(product_id: &str, is_box: bool, options: &[(String, String, f64)]) -> String {
    let mut ids: Vec<&str> = options.iter().map(|o| o.0.as_str()).collect();
    ids.sort();
    format!("{product_id}|{}|{}", if is_box { "BOX" } else { "UNIT" }, ids.join(","))
}

fn add_line(ctx: &AppCtx, product_id: &str, is_box: bool, options: Vec<(String, String, f64)>, qty: i64) {
    with_state(|s| {
        let Some(p) = s.products.iter().find(|p| p.id == product_id) else { return };
        let is_box = is_box && p.sells_box();
        let options = if is_box { Vec::new() } else { options };
        let key = cart_key(product_id, is_box, &options);
        if let Some(item) = s.cart.iter_mut().find(|i| i.key == key) {
            item.qty += qty;
            return;
        }
        let notes = is_box.then(|| format!("Caixa Fechada ({} un)", p.box_units()));
        s.cart.push(CartItem { key, product_id: product_id.to_string(), is_box, options, qty, notes });
    });
    render_quick(ctx);
}

/// Produto com adicionais abre a escolha antes do carrinho. Pelo leitor só abre se houver pergunta
/// obrigatória, para não travar a venda (opcionais: tocar no produto).
fn add_product(ctx: &AppCtx, product_id: &str, is_box: bool, from_scanner: bool, qty: i64) {
    let needs_picker = with_state(|s| {
        s.products.iter().find(|p| p.id == product_id).is_some_and(|p| {
            !(is_box && p.sells_box()) && if from_scanner { p.has_required_modifiers } else { p.has_modifiers }
        })
    });
    if !needs_picker {
        add_line(ctx, product_id, is_box, Vec::new(), qty);
        return;
    }
    let db = ctx.db.clone();
    let pid = product_id.to_string();
    let c = ctx.clone();
    ctx.spawn(async move { catalog::product_modifiers(&db, &pid).await.map(|g| (pid, g)) }, move |ui, result| match result {
        Ok((pid, groups)) if !groups.is_empty() => {
            let selected = groups.iter().map(|(_, g)| vec![false; g.options.len()]).collect();
            with_state(|s| s.picker = Some(Picker { product_id: pid, qty, groups, selected, show_errors: false }));
            ui.global::<CaixaStore>().set_picker_visible(true);
            render_picker(&c);
        }
        Ok((pid, _)) => add_line(&c, &pid, false, Vec::new(), qty),
        Err(e) => alert(ui, e.to_string()),
    });
}

fn selected_ids(p: &Picker) -> Vec<String> {
    p.groups
        .iter()
        .zip(&p.selected)
        .flat_map(|((_, g), sel)| g.options.iter().zip(sel).filter(|(_, on)| **on).map(|(o, _)| o.id.clone()))
        .collect()
}

fn group_rule(g: &ModifierGroup) -> String {
    if g.min_select > 0 {
        if g.max_select == 1 {
            "OBRIGATÓRIO · ESCOLHA 1".into()
        } else if g.max_select > 0 {
            format!("OBRIGATÓRIO · MÍNIMO {}, ATÉ {}", g.min_select, g.max_select)
        } else {
            format!("OBRIGATÓRIO · MÍNIMO {}", g.min_select)
        }
    } else if g.max_select > 0 {
        format!("OPCIONAL · ATÉ {}", g.max_select)
    } else {
        "OPCIONAL".into()
    }
}

fn render_picker(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    let data = with_state(|s| {
        let p = s.picker.as_ref()?;
        let product = s.products.iter().find(|x| x.id == p.product_id)?;
        let base = unit_price(product, &s.promotions).0;
        let ids = selected_ids(p);
        let error = resolve_modifiers(&product.name, &p.groups.iter().map(|(_, g)| g.clone()).collect::<Vec<_>>(), Some(&ids))
            .err()
            .map(|e| e.0);
        let groups: Vec<CxGroup> = p
            .groups
            .iter()
            .zip(&p.selected)
            .map(|((_, g), sel)| {
                let count = sel.iter().filter(|x| **x).count() as i64;
                CxGroup {
                    name: g.name.clone().into(),
                    rule: group_rule(g).into(),
                    required: g.min_select > 0,
                    missing: p.show_errors && count < g.min_select,
                    options: model(
                        g.options
                            .iter()
                            .zip(sel)
                            .map(|(o, on)| CxOption {
                                name: o.name.clone().into(),
                                price: if o.price > 0.0 { format!("+ {}", brl(o.price)).into() } else { SharedString::new() },
                                selected: *on,
                            })
                            .collect(),
                    ),
                }
            })
            .collect();
        let extra: f64 = p
            .groups
            .iter()
            .zip(&p.selected)
            .flat_map(|((_, g), sel)| g.options.iter().zip(sel).filter(|(_, on)| **on).map(|(o, _)| o.price))
            .sum();
        Some((product.name.clone(), groups, brl(base + extra), if p.show_errors { error } else { None }))
    });
    let Some((title, groups, total, error)) = data else { return };
    store.set_picker_title(title.into());
    store.set_picker_groups(model(groups));
    store.set_picker_total(total.into());
    store.set_picker_error(error.unwrap_or_default().into());
}

fn picker_toggle(ctx: &AppCtx, g: usize, o: usize) {
    with_state(|s| {
        let Some(p) = s.picker.as_mut() else { return };
        let (Some((_, group)), Some(sel)) = (p.groups.get(g), p.selected.get_mut(g)) else { return };
        if o >= sel.len() {
            return;
        }
        if sel[o] {
            sel[o] = false;
        } else if group.max_select == 1 {
            // Escolha única: troca a opção em vez de bloquear
            sel.iter_mut().for_each(|x| *x = false);
            sel[o] = true;
        } else if group.max_select > 0 && sel.iter().filter(|x| **x).count() as i64 >= group.max_select {
            // já no máximo
        } else {
            sel[o] = true;
        }
    });
    render_picker(ctx);
}

fn picker_confirm(ctx: &AppCtx) {
    let outcome = with_state(|s| {
        let p = s.picker.as_mut()?;
        let name = s.products.iter().find(|x| x.id == p.product_id).map(|x| x.name.clone()).unwrap_or_default();
        let ids = selected_ids(p);
        let groups: Vec<ModifierGroup> = p.groups.iter().map(|(_, g)| g.clone()).collect();
        if resolve_modifiers(&name, &groups, Some(&ids)).is_err() {
            p.show_errors = true;
            return Some(None);
        }
        let options: Vec<(String, String, f64)> = p
            .groups
            .iter()
            .zip(&p.selected)
            .flat_map(|((_, g), sel)| {
                g.options.iter().zip(sel).filter(|(_, on)| **on).map(|(o, _)| (o.id.clone(), o.name.clone(), o.price))
            })
            .collect();
        let (pid, qty) = (p.product_id.clone(), p.qty);
        s.picker = None;
        Some(Some((pid, options, qty)))
    });
    match outcome {
        Some(Some((pid, options, qty))) => {
            if let Some(ui) = ctx.ui() {
                let store = ui.global::<CaixaStore>();
                store.set_picker_visible(false);
                store.set_focus_search_tick(store.get_focus_search_tick() + 1);
            }
            add_line(ctx, &pid, false, options, qty);
        }
        Some(None) => render_picker(ctx),
        None => {}
    }
}

/// Enter na busca: EAN da caixa adiciona caixa; EAN ou código exato adiciona a unidade; senão o primeiro
/// resultado. Com a escolha de adicionais aberta, um novo bipe não troca o produto.
fn search_enter(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    if with_state(|s| s.picker.is_some()) {
        return;
    }
    let search = store.get_search().trim().to_lowercase();
    let (qty, raw) = split_qty(&search);
    if raw.is_empty() {
        return;
    }
    let clean = raw.strip_prefix('#').unwrap_or(raw).to_string();
    let same = |v: &Option<String>| v.as_deref().is_some_and(|x| x.to_lowercase() == raw || x.to_lowercase() == clean);
    let hit = with_state(|s| {
        if let Some(p) = s.products.iter().find(|p| p.has_box_price && same(&p.box_ean)) {
            return Some((p.id.clone(), true));
        }
        s.products.iter().find(|p| same(&p.ean) || same(&p.code)).map(|p| (p.id.clone(), false))
    });
    let hit = hit.or_else(|| filtered_ids(&store).first().map(|id| (id.clone(), false)));
    if let Some((id, is_box)) = hit {
        store.set_search(SharedString::new());
        add_product(ctx, &id, is_box, true, qty);
    }
}

/// Teclado numérico na tela para o valor entregue em dinheiro.
fn keypad(ctx: &AppCtx, key: &str) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    let mut text = store.get_tendered().to_string();
    match key {
        "back" => {
            text.pop();
        }
        "clear" => text.clear(),
        "," if text.contains(',') => {}
        "," if text.is_empty() => text.push_str("0,"),
        // Até duas casas depois da vírgula
        _ if text.split_once(',').is_some_and(|(_, cents)| cents.len() + key.len() > 2) => {}
        _ => text.push_str(key),
    }
    store.set_tendered(text.into());
    render_quick(ctx);
}

fn finish_sale(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<CaixaStore>();
    if !store.get_quick_visible() || with_state(|s| s.cart.is_empty() || s.submitting || s.picker.is_some()) {
        return;
    }
    let (_, discount, total) = cart_totals(&store);
    let method = store.get_method().to_string();
    let emit_nfce = store.get_emit_nfce();
    let doc_raw = store.get_nfce_doc().to_string();
    let customer_doc: String = doc_raw.chars().filter(char::is_ascii_digit).collect();

    // Conferências da NFC-e antes de gravar a venda (depois de gravada, só a nota poderia falhar)
    if emit_nfce {
        if let Some(err) = cpf_cnpj_error(&doc_raw) {
            alert(&ui, format!("{err} Corrija o CPF/CNPJ da nota ou deixe em branco."));
            return;
        }
        if total <= 0.0 {
            alert(&ui, "Venda com total zero não pode ter NFC-e. Desligue a NFC-e (F8) para concluir.");
            return;
        }
        if customer_doc.is_empty() && total >= NFCE_LIMITE_SEM_IDENTIFICACAO {
            alert(&ui, "Para NFC-e a partir de R$ 10.000,00 a SEFAZ exige o CPF ou CNPJ do consumidor.");
            return;
        }
        let missing: Vec<String> = with_state(|s| {
            let mut names: Vec<String> = Vec::new();
            for p in s.cart.iter().filter_map(|i| s.products.iter().find(|p| p.id == i.product_id)) {
                if !p.valid_ncm() && !names.contains(&p.name) {
                    names.push(p.name.clone());
                }
            }
            names
        });
        if !missing.is_empty() {
            alert(
                &ui,
                format!(
                    "Sem NCM no cadastro: {}.\n\nCadastre o NCM (8 dígitos) em Cardápio & Estoque ou desligue a NFC-e (F8) para concluir sem nota.",
                    missing.join(", ")
                ),
            );
            return;
        }
    }

    let tendered = parse_amount(&store.get_tendered()).filter(|v| *v > 0.0).unwrap_or(total);
    let customer = store.get_customer().trim().to_string();
    let customer_name = if customer.is_empty() { "Cliente Balcão".to_string() } else { customer };
    let items: Vec<NewLine> = with_state(|s| {
        s.cart
            .iter()
            .map(|i| NewLine {
                product_id: i.product_id.clone(),
                quantity: i.qty,
                notes: i.notes.clone(),
                unit_type: Some(if i.is_box { "BOX" } else { "UNIT" }.into()),
                modifier_option_ids: Some(i.options.iter().map(|o| o.0.clone()).collect()),
            })
            .collect()
    });
    let input = QuickSaleInput {
        items,
        payment: QuickSalePayment {
            method: method.clone(),
            amount: Some(total),
            cash_tendered: Some(if method == "CASH" { tendered } else { total }),
            notes: None,
        },
        customer_name: Some(customer_name.clone()),
        discount: Some(discount),
        expected_total: None,
    };
    let print_receipt = store.get_print_receipt();
    with_state(|s| s.submitting = true);
    store.set_submitting(true);

    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move { orders::quick_sale(&db, input, Local::now().naive_local()).await },
        move |ui, result| {
            with_state(|s| s.submitting = false);
            let store = ui.global::<CaixaStore>();
            store.set_submitting(false);
            match result {
                Ok((res, events)) => {
                    c.emit(&events);
                    let order = &res.order;
                    store.set_success_text(format!("Venda #{} finalizada com sucesso!", order.head.order_number).into());
                    store.set_success_change(if res.change > 0.0 { brl(res.change).into() } else { SharedString::new() });

                    // Com NFC-e o cupom fiscal substitui o comprovante; se a nota falhar, sai o comprovante (se marcado)
                    if emit_nfce {
                        let venda = venda_nfce(order, &customer_doc, &customer_name, &method);
                        run_nfce(&c, venda, print_receipt.then(|| order.head.id.clone()));
                    } else if print_receipt {
                        print_receipt_of(&c, &order.head.id);
                    }

                    with_state(|s| s.cart.clear());
                    store.set_discount(SharedString::new());
                    store.set_tendered(SharedString::new());
                    store.set_customer(SharedString::new());
                    store.set_nfce_doc(SharedString::new());
                    render_quick(&c);
                    store.set_focus_search_tick(store.get_focus_search_tick() + 1);
                    refresh(&c);
                }
                Err(e) => {
                    render_quick(&c);
                    alert(ui, e.to_string());
                }
            }
        },
    );
}

fn print_receipt_of(ctx: &AppCtx, order_id: &str) {
    let db = ctx.db.clone();
    let id = order_id.to_string();
    let c = ctx.clone();
    ctx.spawn(async move { crate::print::print_order_receipt(&db, &id).await }, move |_, r| match r {
        Ok(msg) => c.status(msg, false),
        Err(e) => c.status(format!("Comprovante não impresso: {e}"), true),
    });
}

/// Venda gravada no formato do `emitNfceForOrder` (itens com o preço cobrado e os adicionais no nome).
fn venda_nfce(order: &OrderDetail, customer_doc: &str, customer_name: &str, method: &str) -> VendaNfce {
    let itens = with_state(|s| {
        order
            .items
            .iter()
            .rev() // o detalhe vem do mais novo para o mais antigo; a nota segue a ordem lançada
            .map(|i| {
                let p = s.products.iter().find(|p| p.id == i.product_id);
                let is_box = i.unit_type.as_deref() == Some("BOX");
                let mods: Vec<String> = i
                    .modifiers
                    .as_deref()
                    .and_then(|m| serde_json::from_str::<Vec<serde_json::Value>>(m).ok())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect();
                let nome = if mods.is_empty() { i.product_name.clone() } else { format!("{} ({})", i.product_name, mods.join(", ")) };
                let field = |f: fn(&PdvProduct) -> &Option<String>| p.and_then(|p| f(p).clone()).unwrap_or_default();
                let ean = if is_box {
                    p.and_then(|p| p.box_ean.clone().filter(|e| !e.is_empty()).or_else(|| p.ean.clone())).unwrap_or_default()
                } else {
                    field(|p| &p.ean)
                };
                ItemVendaNfce {
                    product_id: i.product_id.clone(),
                    ean,
                    codigo: field(|p| &p.code),
                    quantidade: i.quantity as f64,
                    preco_unitario: i.unit_price,
                    total: i.total_price,
                    nome,
                    ncm: field(|p| &p.ncm).chars().filter(char::is_ascii_digit).collect(),
                    cfop: Some(field(|p| &p.cfop)).filter(|c| !c.is_empty()).unwrap_or_else(|| "5102".into()),
                    unidade: if is_box { "CX".into() } else { Some(field(|p| &p.unit)).filter(|u| !u.is_empty()).unwrap_or_else(|| "un".into()) },
                    cest: field(|p| &p.cest),
                }
            })
            .collect()
    });
    VendaNfce {
        sale_key: order.head.id.clone(),
        numero_pedido: order.head.order_number,
        documento: customer_doc.to_string(),
        nome_cliente: customer_name.to_string(),
        forma_pagamento: method.to_string(),
        desconto: order.head.discount,
        itens,
    }
}

/// Emite a NFC-e da venda já gravada. Falha da SEFAZ não desfaz a venda (dá para tentar de novo).
/// `fallback_receipt`: comanda cujo comprovante sai se a nota falhar.
fn run_nfce(ctx: &AppCtx, venda: VendaNfce, fallback_receipt: Option<String>) {
    let number = venda.numero_pedido;
    with_state(|s| {
        s.nfce.retain(|e| e.order_number != number);
        s.nfce.insert(
            0,
            NfceEntry { order_number: number, kind: NfceKind::Emitting, message: String::new(), referencia: None, venda: venda.clone() },
        );
        s.nfce.truncate(4);
    });
    render_nfce(ctx);
    let c = ctx.clone();
    fiscal_page::emitir_nfce_da_venda(ctx, venda, move |_, result| {
        let (kind, message, referencia) = match result {
            ResultadoNfce::Autorizada { referencia } => {
                fiscal_page::imprimir_danfe_nfce(&c, &referencia);
                (NfceKind::Authorized, String::new(), Some(referencia))
            }
            ResultadoNfce::Processando { referencia } => (
                NfceKind::Processing,
                format!("Em processamento na SEFAZ (ref: {referencia}). NÃO emita de novo: consulte em Reimpressão de NFC-e."),
                Some(referencia),
            ),
            ResultadoNfce::Erro(msg) => {
                if let Some(order_id) = &fallback_receipt {
                    print_receipt_of(&c, order_id);
                }
                (NfceKind::Error, if msg.is_empty() { "Falha ao emitir a NFC-e.".into() } else { msg }, None)
            }
        };
        with_state(|s| {
            if let Some(e) = s.nfce.iter_mut().find(|e| e.order_number == number) {
                e.kind = kind;
                e.message = message;
                e.referencia = referencia;
            }
        });
        render_nfce(&c);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_like_parse_float() {
        assert_eq!(parse_amount("200.00"), Some(200.0));
        assert_eq!(parse_amount("12,5"), Some(12.5));
        assert_eq!(parse_amount(" 7 "), Some(7.0));
        assert_eq!(parse_amount("10abc"), Some(10.0));
        assert_eq!(parse_amount("abc"), None);
        assert_eq!(parse_amount(""), None);
    }

    #[test]
    fn quantity_prefix() {
        assert_eq!(split_qty("3*7891000"), (3, "7891000"));
        assert_eq!(split_qty("12 * hein"), (12, "hein"));
        assert_eq!(split_qty("hein"), (1, "hein"));
        assert_eq!(split_qty("0*hein"), (1, "0*hein"));
        assert_eq!(split_qty("x*hein"), (1, "x*hein"));
    }

    #[test]
    fn cart_key_ignores_option_order() {
        let a = vec![("b".into(), "B".into(), 1.0), ("a".into(), "A".into(), 0.0)];
        let b = vec![("a".into(), "A".into(), 0.0), ("b".into(), "B".into(), 1.0)];
        assert_eq!(cart_key("p", false, &a), cart_key("p", false, &b));
        assert_ne!(cart_key("p", false, &a), cart_key("p", true, &[]));
    }

    #[test]
    fn signed_difference() {
        assert_eq!(signed_brl(1.5), "+ R$ 1,50");
        assert_eq!(signed_brl(0.0), "R$ 0,00");
        assert_eq!(signed_brl(-2.0), "-R$ 2,00");
    }
}
