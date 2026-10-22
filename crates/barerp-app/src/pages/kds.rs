//! Página "kds": KdsView.tsx (fila de produção do bar e da cozinha). Ver `paridade/kds.md`.
//!
//! Os itens vêm de `barerp_db::kds::queue` e são agrupados por comanda (um ticket por mesa), do mais antigo
//! para o mais novo (FIFO). A tela recarrega ao entrar, a cada 15 s (contingência do original) e na hora em
//! que chega um evento `kds:*`/`order:*` dos celulares.
//!
//! Modo cozinha: as comandas 1–9 da fila têm tecla (PRONTO), Backspace desfaz a última liberada, o total
//! por produto aparece no alto e um bipe avisa quando entra item novo enquanto a tela está aberta.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{Local, Utc};
use slint::{ComponentHandle, ModelRc, VecModel};

use barerp_db::kds::{self, KdsItem};

use crate::ctx::{self, AppCtx, Preferences};
use crate::{AppWindow, KdsLine, KdsRow, KdsStore, KdsSum, KdsTicket, MesasStore, Page};

#[derive(Default)]
struct State {
    items: Vec<KdsItem>,
    cols: usize,
    last_load: Option<Instant>,
    /// Itens já vistos (para o aviso sonoro); `None` antes da primeira carga
    known: Option<HashSet<String>>,
    /// Última comanda liberada com PRONTO: itens e o status de antes, para o Desfazer
    last_ready: Vec<(String, String)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { cols: 1, ..State::default() });
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// Praça do filtro para o banco (`ALL` = todas).
fn station_param(station: &str) -> Option<String> {
    (station != "ALL").then(|| station.to_string())
}

/// Tickets (um por comanda) do mais antigo para o mais novo, como o `useMemo` do original.
fn tickets(items: &[KdsItem]) -> Vec<KdsTicket> {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, Vec<&KdsItem>> = HashMap::new();
    for it in items {
        if !grouped.contains_key(&it.order_id) {
            order.push(it.order_id.clone());
        }
        grouped.entry(it.order_id.clone()).or_default().push(it);
    }
    let now = Utc::now();
    let mut list: Vec<(i64, KdsTicket)> = order
        .into_iter()
        .map(|order_id| {
            let group = &grouped[&order_id];
            let first = group[0];
            let oldest = group.iter().map(|i| i.added_at.0).min().unwrap_or(first.added_at.0);
            let minutes = (now - oldest).num_minutes().max(0);
            // Mesma ordem de preferência do original: nome da mesa, "Mesa N" ou "Balcão"
            let place = match (first.table_name.as_deref().filter(|n| !n.is_empty()), first.table_number) {
                (Some(name), _) => name.to_string(),
                (None, Some(n)) => format!("Mesa {n}"),
                (None, None) => "Balcão".to_string(),
            };
            let customer = first.customer_name.as_deref().filter(|c| !c.trim().is_empty()).unwrap_or("Sem nome");
            let lines = group
                .iter()
                .map(|i| KdsLine {
                    id: i.id.clone().into(),
                    qty: i.quantity as i32,
                    name: i.product_name.clone().into(),
                    station: if i.kds_station == "BAR" { "🍺 Bar" } else { "🍳 Cozinha" }.into(),
                    notes: i.notes.clone().unwrap_or_default().into(),
                    status: i.kds_status.clone().into(),
                })
                .collect();
            let ticket = KdsTicket {
                order_id: order_id.clone().into(),
                place: place.into(),
                sub: format!("CMD #{} • {customer}", first.order_number).into(),
                minutes: minutes as i32,
                index: 0,
                items: model(lines),
            };
            (oldest.timestamp_millis(), ticket)
        })
        .collect();
    list.sort_by_key(|(oldest, _)| *oldest);
    list.into_iter()
        .enumerate()
        .map(|(i, (_, mut t))| {
            t.index = if i < 9 { i as i32 + 1 } else { 0 };
            t
        })
        .collect()
}

/// Total por produto do que ainda falta fazer (pendente ou em preparo), do maior para o menor.
fn summary(items: &[KdsItem]) -> Vec<KdsSum> {
    let mut totals: Vec<(String, i64)> = Vec::new();
    for it in items.iter().filter(|i| i.kds_status == "PENDING" || i.kds_status == "PREPARING") {
        match totals.iter_mut().find(|(name, _)| *name == it.product_name) {
            Some((_, qty)) => *qty += it.quantity,
            None => totals.push((it.product_name.clone(), it.quantity)),
        }
    }
    totals.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    totals.into_iter().map(|(name, qty)| KdsSum { qty: qty as i32, name: name.into() }).collect()
}

/// Divide os tickets em linhas de `cols` (a última linha completa com espaço vazio).
fn render(ui: &AppWindow) {
    let (items, cols) = with_state(|s| (s.items.clone(), s.cols.max(1)));
    let store = ui.global::<KdsStore>();
    store.set_count(items.len() as i32);
    let rows = tickets(&items)
        .chunks(cols)
        .map(|chunk| KdsRow { tickets: model(chunk.to_vec()), empty: (cols - chunk.len()) as i32 })
        .collect();
    store.set_rows(model(rows));
    store.set_summary(model(summary(&items)));
}

/// Guarda a fila nova e bipa se entrou item que ainda não estava na tela.
fn store_items(ui: &AppWindow, items: Vec<KdsItem>) {
    let ids: HashSet<String> = items.iter().map(|i| i.id.clone()).collect();
    let fresh = with_state(|s| {
        let fresh = s.known.as_ref().is_some_and(|known| {
            items.iter().any(|i| i.kds_status == "PENDING" && !known.contains(&i.id))
        });
        s.known = Some(ids);
        s.items = items;
        fresh
    });
    if fresh && ui.global::<KdsStore>().get_sound() {
        ctx::beep();
    }
}

/// Recarrega a fila da praça escolhida (e o contador da aba, que conta todas as praças).
pub fn refresh(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<KdsStore>();
    store.set_loading(true);
    store.set_connected(ui.get_server_online());
    with_state(|s| s.last_load = Some(Instant::now()));
    let station = station_param(&store.get_station());
    let db = ctx.db.clone();
    ctx.spawn(
        async move {
            let items = kds::queue(&db, station.as_deref()).await?;
            let total = if station.is_some() { kds::queue(&db, None).await?.len() } else { items.len() };
            Ok::<_, barerp_db::DbError>((station, items, total))
        },
        |ui, result| {
            let store = ui.global::<KdsStore>();
            store.set_loading(false);
            match result {
                // Ignora a resposta de uma praça que já não está selecionada (cliques rápidos no seletor)
                Ok((station, _, _)) if station != station_param(&store.get_station()) => {}
                Ok((_, items, total)) => {
                    store_items(ui, items);
                    render(ui);
                    ui.set_kds_total(total as i32);
                }
                Err(e) => tracing::warn!("erro ao carregar KDS: {e}"),
            }
        },
    );
}

/// Só o número da aba "KDS (Cozinha)" (todas as praças), fora da tela do KDS.
pub fn refresh_badge(ctx: &AppCtx) {
    let db = ctx.db.clone();
    ctx.spawn(async move { kds::queue(&db, None).await }, |ui, result| {
        if let Ok(items) = result {
            ui.set_kds_total(items.len() as i32);
        }
    });
}

/// PRONTO numa comanda: marca os itens da praça como prontos e guarda o status de antes para o Desfazer.
fn ready_all(c: &AppCtx, order_id: String) {
    let Some(ui) = c.ui() else { return };
    let station = station_param(&ui.global::<KdsStore>().get_station());
    let previous: Vec<(String, String)> = with_state(|s| {
        s.items
            .iter()
            .filter(|i| i.order_id == order_id && i.kds_status != "READY")
            .map(|i| (i.id.clone(), i.kds_status.clone()))
            .collect()
    });
    let db = c.db.clone();
    let c2 = c.clone();
    c.spawn(async move { kds::ready_all(&db, &order_id, station.as_deref()).await }, move |ui, result| match result {
        Ok(events) => {
            with_state(|s| s.last_ready = previous);
            ui.global::<KdsStore>().set_can_undo(true);
            c2.emit(&events);
            refresh(&c2);
        }
        Err(e) => alert(
            &c2,
            if e.to_string().is_empty() { "Erro ao marcar todos como prontos".into() } else { e.to_string() },
        ),
    });
}

fn alert(ctx: &AppCtx, text: String) {
    if let Some(ui) = ctx.ui() {
        ui.global::<KdsStore>().set_alert_text(text.into());
    }
}

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<KdsStore>();

    let c = ctx.clone();
    store.on_refresh(move || refresh(&c));

    let c = ctx.clone();
    store.on_set_station(move |station| {
        if let Some(ui) = c.ui() {
            ui.global::<KdsStore>().set_station(station);
        }
        with_state(|s| s.known = None);
        refresh(&c);
    });

    store.on_cols_changed({
        let weak = ui.as_weak();
        move |cols| {
            with_state(|s| s.cols = cols.max(1) as usize);
            if let Some(ui) = weak.upgrade() {
                render(&ui);
            }
        }
    });

    // Atualizar status individual
    let c = ctx.clone();
    store.on_advance(move |item_id, next| {
        let db = c.db.clone();
        let (item_id, next) = (item_id.to_string(), next.to_string());
        let c2 = c.clone();
        c.spawn(async move { kds::set_item_status(&db, &item_id, &next).await }, move |_, result| match result {
            Ok((_, events)) => {
                c2.emit(&events);
                refresh(&c2);
            }
            Err(e) => alert(&c2, if e.to_string().is_empty() { "Erro ao atualizar status".into() } else { e.to_string() }),
        });
    });

    // Marcar todos do ticket como prontos (na praça escolhida), pelo botão ou pela tecla 1–9
    let c = ctx.clone();
    store.on_ready_all(move |order_id| ready_all(&c, order_id.to_string()));

    let c = ctx.clone();
    store.on_ready_index(move |index| {
        let order = with_state(|s| tickets(&s.items).into_iter().find(|t| t.index == index).map(|t| t.order_id));
        if let Some(order_id) = order {
            ready_all(&c, order_id.to_string());
        }
    });

    // Desfazer: volta os itens da última comanda liberada ao status de antes
    let c = ctx.clone();
    store.on_undo(move || {
        let previous = with_state(|s| std::mem::take(&mut s.last_ready));
        if let Some(ui) = c.ui() {
            ui.global::<KdsStore>().set_can_undo(false);
        }
        if previous.is_empty() {
            return;
        }
        let db = c.db.clone();
        let c2 = c.clone();
        c.spawn(
            async move {
                let mut events = Vec::new();
                for (item_id, status) in previous {
                    events.extend(kds::set_item_status(&db, &item_id, &status).await?.1);
                }
                Ok::<_, barerp_db::DbError>(events)
            },
            move |_, result| match result {
                Ok(events) => {
                    c2.emit(&events);
                    refresh(&c2);
                }
                Err(e) => alert(&c2, format!("Não foi possível desfazer: {e}")),
            },
        );
    });

    let c = ctx.clone();
    store.on_toggle_sound(move || {
        let mut prefs = Preferences::load();
        prefs.kds_sound = !prefs.kds_sound;
        prefs.save();
        if let Some(ui) = c.ui() {
            ui.global::<KdsStore>().set_sound(prefs.kds_sound);
        }
    });

    // Wi-Fi / QR: no PC do caixa o servidor é este app; o botão mostra o QR para os celulares se conectarem
    let c = ctx.clone();
    store.on_open_connect(move || {
        if let Some(ui) = c.ui() {
            ui.global::<MesasStore>().invoke_open_connect_mobile();
        }
    });

    // Polling de contingência a cada 15 s enquanto a tela está aberta (e ao entrar nela)
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), {
        let c = ctx.clone();
        let mut was_open = false;
        move || {
            let Some(ui) = c.ui() else { return };
            let open = ui.get_page() == Page::Kds;
            if open {
                ui.global::<KdsStore>().set_clock(Local::now().format("%H:%M").to_string().into());
            }
            let due = with_state(|s| s.last_load.is_none_or(|t| t.elapsed() >= Duration::from_secs(15)));
            if open && (!was_open || due) {
                refresh(&c);
            }
            was_open = open;
        }
    });
    TIMER.with(|t| *t.borrow_mut() = Some(timer));

    // Pedidos e mudanças vindos dos celulares: atualiza a fila (ou só o número da aba) na hora
    if let Some(server) = &ctx.server {
        let mut events = server.subscribe();
        let c = ctx.clone();
        ctx.rt.spawn(async move {
            loop {
                let name = match events.recv().await {
                    Ok(ev) => ev.name,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if name.starts_with("kds:") || name.starts_with("order:") {
                    let c2 = c.clone();
                    let _ = c.ui.upgrade_in_event_loop(move |ui| {
                        if ui.get_page() == Page::Kds {
                            refresh(&c2);
                        } else {
                            refresh_badge(&c2);
                        }
                    });
                }
            }
        });
    }
    refresh_badge(ctx);
}

/// Para os prints das telas (`examples/kds_shots.rs`), sem banco nem servidor.
pub mod preview {
    use super::*;

    pub fn show(ui: &AppWindow, items: Vec<KdsItem>, cols: usize) {
        with_state(|s| {
            s.items = items;
            s.cols = cols;
        });
        render(ui);
    }
}
