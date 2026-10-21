//! Lançar pedido na mesa (AddOrderModal + ModifierPickerModal).

use chrono::Local;
use slint::{ComponentHandle, SharedString};

use barerp_core::money::brl;
use barerp_core::pricing::{self, ModifierGroup, Promotion};
use barerp_db::catalog::{self, Category, SaleProduct};
use barerp_db::orders::{self, NewLine};
use barerp_db::DbResult;

use super::{card, close_modal, modal, modal_error, refresh, table_title, with_state};
use crate::ctx::AppCtx;
use crate::view::{fold, model};
use crate::{AddCart, AddProduct, Choice, MesasModal, MesasModalStore, ModGroup};

const DEFAULT_BOX: i64 = 24;

#[derive(Clone)]
pub(crate) struct LaunchItem {
    product_id: String,
    name: String,
    is_box: bool,
    box_quantity: i64,
    option_ids: Vec<String>,
    options: String,
    promotion: String,
    unit_price: f64,
    quantity: i64,
    notes: String,
}

impl LaunchItem {
    fn same(&self, other: &LaunchItem) -> bool {
        let mut a = self.option_ids.clone();
        let mut b = other.option_ids.clone();
        a.sort();
        b.sort();
        self.product_id == other.product_id && self.is_box == other.is_box && a == b
    }
}

struct Pending {
    product_id: String,
    groups: Vec<(String, ModifierGroup)>,
    selected: Vec<Vec<bool>>,
}

#[derive(Default)]
pub(crate) struct Launch {
    order_id: String,
    products: Vec<SaleProduct>,
    categories: Vec<Category>,
    promotions: Vec<Promotion>,
    cart: Vec<LaunchItem>,
    pending: Option<Pending>,
}

fn now() -> chrono::NaiveDateTime {
    Local::now().naive_local()
}

fn render_products(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    let category = m.get_a_category().to_string();
    let query = fold(m.get_a_search().trim());
    let items: Vec<AddProduct> = with_state(|s| {
        let l = &s.launch;
        l.products
            .iter()
            .filter(|p| category == "ALL" || p.category_id == category)
            .filter(|p| query.is_empty() || fold(&p.name).contains(&query))
            .map(|p| {
                let (price, promo) = pricing::apply_best_promotion(p.price, &p.id, &p.category_id, &l.promotions, now());
                let count = |is_box: bool| {
                    l.cart.iter().filter(|c| c.product_id == p.id && c.is_box == is_box).map(|c| c.quantity).sum::<i64>() as i32
                };
                let has_box = p.has_box_price && p.box_price.is_some_and(|b| b > 0.0);
                AddProduct {
                    id: p.id.clone().into(),
                    station: if p.kds_station == "BAR" { "Bar".into() } else { "Cozinha".into() },
                    name: p.name.clone().into(),
                    description: p.description.clone().unwrap_or_default().into(),
                    price: brl(price).into(),
                    base_price: if promo.is_some() { brl(p.price).into() } else { SharedString::new() },
                    promotion: promo.map(|x| x.name.clone()).unwrap_or_default().into(),
                    has_options: p.has_modifiers,
                    has_box,
                    box_label: format!("Cx {}x", p.box_quantity.unwrap_or(DEFAULT_BOX)).into(),
                    box_price: brl(p.box_price.unwrap_or(0.0)).into(),
                    unit_count: count(false),
                    box_count: count(true),
                }
            })
            .collect()
    });
    m.set_a_products(model(items));
}

fn render_cart(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    let (rows, total): (Vec<AddCart>, f64) = with_state(|s| {
        let rows = s
            .launch
            .cart
            .iter()
            .map(|c| AddCart {
                name: c.name.clone().into(),
                is_box: c.is_box,
                box_label: format!("Cx {}x", c.box_quantity).into(),
                options: c.options.clone().into(),
                promotion: c.promotion.clone().into(),
                total: brl(c.unit_price * c.quantity as f64).into(),
                quantity: c.quantity as i32,
                notes: c.notes.clone().into(),
            })
            .collect();
        let total = s.launch.cart.iter().map(|c| c.unit_price * c.quantity as f64).sum();
        (rows, total)
    });
    m.set_a_cart(model(rows));
    m.set_a_total(brl(total).into());
    render_products(ui);
}

/// Abre o lançamento para a mesa em foco.
pub(crate) fn open(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let Some(table_id) = with_state(|s| s.table_id.clone()) else { return };
    let Some(card) = card(&table_id) else { return };
    let Some(order_id) = card.order_id.clone() else {
        ctx.status("Esta mesa não tem comanda aberta.", true);
        return;
    };
    let m = ui.global::<MesasModalStore>();
    let number = with_state(|s| s.detail.as_ref().map(|d| d.head.order_number));
    m.set_a_order(number.map(|n| format!("Comanda #{n}")).unwrap_or_default().into());
    let mut title = table_title(&card);
    if let Some(customer) = card.customer_name.clone().filter(|c| !c.is_empty()) {
        title.push_str(&format!(" • {customer}"));
    }
    m.set_a_title(title.into());
    m.set_a_category("ALL".into());
    m.set_a_search(SharedString::new());
    m.set_a_loading(true);
    with_state(|s| {
        if s.launch.order_id != order_id {
            s.launch.cart.clear();
        }
        s.launch.order_id = order_id.clone();
    });
    render_cart(&ui);
    modal(&ui, MesasModal::AddOrder);

    // Número da comanda quando o lançamento vem direto do cartão
    if number.is_none() {
        let db = ctx.db.clone();
        let id = order_id.clone();
        ctx.spawn(async move { orders::detail(&db, &id).await }, |ui, result| {
            if let Ok(d) = result {
                ui.global::<MesasModalStore>().set_a_order(format!("Comanda #{}", d.head.order_number).into());
            }
        });
    }

    let db = ctx.db.clone();
    ctx.spawn(
        async move {
            let products = catalog::sale_products(&db).await?;
            let categories = catalog::categories(&db).await?;
            let promotions = catalog::promotions(&db).await?;
            DbResult::Ok((products, categories, promotions))
        },
        |ui, result| {
            let m = ui.global::<MesasModalStore>();
            m.set_a_loading(false);
            match result {
                Ok((products, categories, promotions)) => {
                    let cats: Vec<Choice> = categories
                        .iter()
                        .map(|c| Choice {
                            id: c.id.clone().into(),
                            label: c.name.clone().into(),
                            detail: c.icon.clone().unwrap_or_default().into(),
                            selected: false,
                        })
                        .collect();
                    m.set_a_categories(model(cats));
                    with_state(|s| {
                        s.launch.products = products;
                        s.launch.categories = categories;
                        s.launch.promotions = promotions;
                    });
                    render_cart(ui);
                }
                Err(e) => modal_error(ui, format!("Erro ao carregar o cardápio: {e}")),
            }
        },
    );
}

fn product(id: &str) -> Option<SaleProduct> {
    with_state(|s| s.launch.products.iter().find(|p| p.id == id).cloned())
}

/// Coloca no carrinho (soma à linha igual, se houver).
fn add(ui: &crate::AppWindow, p: &SaleProduct, is_box: bool, groups: &[ModifierGroup], option_ids: Vec<String>) {
    let result = with_state(|s| {
        let priced = pricing::price_order_line(
            &pricing::ProductForPricing {
                id: &p.id,
                name: &p.name,
                category_id: &p.category_id,
                price: p.price,
                box_price: p.box_price,
                has_box_price: p.has_box_price,
            },
            is_box.then_some("BOX"),
            groups,
            Some(&option_ids),
            &s.launch.promotions,
            now(),
        )?;
        let options = groups
            .iter()
            .flat_map(|g| g.options.iter())
            .filter(|o| option_ids.contains(&o.id))
            .map(|o| o.name.clone())
            .collect::<Vec<_>>()
            .join(" · ");
        let box_quantity = p.box_quantity.unwrap_or(DEFAULT_BOX);
        let item = LaunchItem {
            product_id: p.id.clone(),
            name: p.name.clone(),
            is_box: priced.is_box,
            box_quantity,
            option_ids: if priced.is_box { Vec::new() } else { option_ids },
            options,
            promotion: priced.promotion_name.clone().unwrap_or_default(),
            unit_price: priced.unit_price,
            quantity: 1,
            notes: if priced.is_box { format!("Caixa Fechada ({box_quantity} un)") } else { String::new() },
        };
        if let Some(existing) = s.launch.cart.iter_mut().find(|c| c.same(&item)) {
            existing.quantity += 1;
        } else {
            s.launch.cart.push(item);
        }
        Ok::<_, pricing::PricingError>(())
    });
    match result {
        Ok(()) => render_cart(ui),
        Err(e) => modal_error(ui, e.0),
    }
}

fn render_modifiers(ui: &crate::AppWindow) {
    let groups: Vec<ModGroup> = with_state(|s| {
        let Some(p) = &s.launch.pending else { return Vec::new() };
        p.groups
            .iter()
            .zip(&p.selected)
            .map(|((_, g), sel)| {
                let chosen = sel.iter().filter(|x| **x).count() as i64;
                let rule = if g.min_select > 0 {
                    if g.min_select == 1 && g.max_select == 1 {
                        "Obrigatório · escolha 1".to_string()
                    } else if g.max_select > 0 {
                        format!("Obrigatório · mínimo {} · até {}", g.min_select, g.max_select)
                    } else {
                        format!("Obrigatório · mínimo {}", g.min_select)
                    }
                } else if g.max_select > 0 {
                    format!("Opcional · até {}", g.max_select)
                } else {
                    "Opcional".to_string()
                };
                ModGroup {
                    name: g.name.clone().into(),
                    rule: rule.into(),
                    missing: chosen < g.min_select,
                    options: model(
                        g.options
                            .iter()
                            .zip(sel)
                            .map(|(o, &on)| Choice {
                                id: o.id.clone().into(),
                                label: o.name.clone().into(),
                                detail: if o.price > 0.0 { format!("+ {}", brl(o.price)).into() } else { SharedString::new() },
                                selected: on,
                            })
                            .collect(),
                    ),
                }
            })
            .collect()
    });
    ui.global::<MesasModalStore>().set_m_groups(model(groups));
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();

    m.on_a_filter({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render_products(&ui);
            }
        }
    });
    m.on_a_add_unit({
        let ctx = ctx.clone();
        move |id| {
            let (Some(ui), Some(p)) = (ctx.ui(), product(&id)) else { return };
            if !p.has_modifiers {
                add(&ui, &p, false, &[], Vec::new());
                return;
            }
            let db = ctx.db.clone();
            let pid = p.id.clone();
            ctx.spawn(async move { catalog::product_modifiers(&db, &pid).await }, move |ui, result| match result {
                Ok(groups) => {
                    let selected = groups.iter().map(|(_, g)| vec![false; g.options.len()]).collect();
                    with_state(|s| s.launch.pending = Some(Pending { product_id: p.id.clone(), groups, selected }));
                    let (price, _) = with_state(|s| {
                        let (price, promo) =
                            pricing::apply_best_promotion(p.price, &p.id, &p.category_id, &s.launch.promotions, now());
                        (price, promo.is_some())
                    });
                    let m = ui.global::<MesasModalStore>();
                    m.set_m_product(p.name.clone().into());
                    m.set_m_price(format!("A partir de {}", brl(price)).into());
                    render_modifiers(ui);
                    modal(ui, MesasModal::Modifiers);
                }
                Err(e) => modal_error(ui, format!("Erro ao carregar os adicionais: {e}")),
            });
        }
    });
    m.on_a_add_box({
        let ctx = ctx.clone();
        move |id| {
            if let (Some(ui), Some(p)) = (ctx.ui(), product(&id)) {
                add(&ui, &p, true, &[], Vec::new());
            }
        }
    });
    m.on_a_qty({
        let ctx = ctx.clone();
        move |index, delta| {
            with_state(|s| {
                let i = index as usize;
                if let Some(item) = s.launch.cart.get_mut(i) {
                    item.quantity += i64::from(delta);
                    if item.quantity <= 0 {
                        s.launch.cart.remove(i);
                    }
                }
            });
            if let Some(ui) = ctx.ui() {
                render_cart(&ui);
            }
        }
    });
    // Observação digitada: guarda sem redesenhar (redesenhar tiraria o foco do campo)
    m.on_a_notes(move |index, text| {
        with_state(|s| {
            if let Some(item) = s.launch.cart.get_mut(index as usize) {
                item.notes = text.to_string();
            }
        });
    });
    m.on_a_chip({
        let ctx = ctx.clone();
        move |index, chip| {
            with_state(|s| {
                if let Some(item) = s.launch.cart.get_mut(index as usize) {
                    item.notes = if item.notes.trim().is_empty() { chip.to_string() } else { format!("{}, {chip}", item.notes) };
                }
            });
            if let Some(ui) = ctx.ui() {
                render_cart(&ui);
            }
        }
    });
    m.on_a_clear({
        let ctx = ctx.clone();
        move || {
            with_state(|s| s.launch.cart.clear());
            if let Some(ui) = ctx.ui() {
                render_cart(&ui);
            }
        }
    });
    m.on_a_send({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let (order_id, lines) = with_state(|s| {
                let lines: Vec<NewLine> = s
                    .launch
                    .cart
                    .iter()
                    .map(|c| NewLine {
                        product_id: c.product_id.clone(),
                        quantity: c.quantity,
                        notes: Some(c.notes.trim().to_string()).filter(|n| !n.is_empty()),
                        unit_type: Some(if c.is_box { "BOX" } else { "UNIT" }.to_string()),
                        modifier_option_ids: Some(c.option_ids.clone()),
                    })
                    .collect();
                (s.launch.order_id.clone(), lines)
            });
            if lines.is_empty() || order_id.is_empty() {
                return;
            }
            ui.global::<MesasModalStore>().set_busy(true);
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { orders::add_items(&db, &order_id, &lines, now()).await }, move |ui, result| match result {
                Ok((_, events)) => {
                    next.emit(&events);
                    with_state(|s| s.launch.cart.clear());
                    close_modal(ui);
                    next.status("Pedido enviado para o bar/cozinha.", false);
                    refresh(&next);
                }
                Err(e) => modal_error(ui, e.to_string()),
            });
        }
    });

    m.on_m_toggle({
        let ctx = ctx.clone();
        move |g, o| {
            with_state(|s| {
                let Some(p) = s.launch.pending.as_mut() else { return };
                let (g, o) = (g as usize, o as usize);
                let Some(max) = p.groups.get(g).map(|(_, grp)| grp.max_select) else { return };
                let sel = &mut p.selected[g];
                let was = sel[o];
                if !was && max == 1 {
                    sel.iter_mut().for_each(|v| *v = false);
                } else if !was && max > 1 && sel.iter().filter(|x| **x).count() as i64 >= max {
                    return;
                }
                sel[o] = !was;
            });
            if let Some(ui) = ctx.ui() {
                render_modifiers(&ui);
            }
        }
    });
    m.on_m_confirm({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let Some((product_id, groups, option_ids)) = with_state(|s| {
                let p = s.launch.pending.as_ref()?;
                let option_ids: Vec<String> = p
                    .groups
                    .iter()
                    .zip(&p.selected)
                    .flat_map(|((_, g), sel)| g.options.iter().zip(sel).filter(|(_, on)| **on).map(|(o, _)| o.id.clone()))
                    .collect();
                Some((p.product_id.clone(), p.groups.iter().map(|(_, g)| g.clone()).collect::<Vec<_>>(), option_ids))
            }) else {
                return;
            };
            let Some(p) = product(&product_id) else { return };
            if let Err(e) = pricing::resolve_modifiers(&p.name, &groups, Some(&option_ids)) {
                modal_error(&ui, e.0);
                return;
            }
            with_state(|s| s.launch.pending = None);
            modal(&ui, MesasModal::AddOrder);
            add(&ui, &p, false, &groups, option_ids);
        }
    });
    m.on_m_cancel({
        let ctx = ctx.clone();
        move || {
            with_state(|s| s.launch.pending = None);
            if let Some(ui) = ctx.ui() {
                modal(&ui, MesasModal::AddOrder);
            }
        }
    });
}

/// Gerador de prints: abre o lançamento com o cardápio já carregado e dois itens no carrinho.
pub(crate) fn preview(
    ui: &crate::AppWindow,
    table_id: &str,
    products: Vec<SaleProduct>,
    categories: Vec<Category>,
    promotions: Vec<Promotion>,
) {
    let Some(card) = card(table_id) else { return };
    let m = ui.global::<MesasModalStore>();
    m.set_a_order("Comanda #101".into());
    m.set_a_title(format!("{} • {}", table_title(&card), card.customer_name.clone().unwrap_or_default()).into());
    m.set_a_category("ALL".into());
    m.set_a_loading(false);
    let cats: Vec<Choice> = categories
        .iter()
        .map(|c| Choice { id: c.id.clone().into(), label: c.name.clone().into(), detail: c.icon.clone().unwrap_or_default().into(), selected: false })
        .collect();
    m.set_a_categories(model(cats));
    let picks: Vec<SaleProduct> = products.iter().take(2).cloned().collect();
    with_state(|s| {
        s.launch = Launch { order_id: "preview".into(), products, categories, promotions, cart: Vec::new(), pending: None };
    });
    for p in &picks {
        add(ui, p, false, &[], Vec::new());
    }
    with_state(|s| {
        if let Some(first) = s.launch.cart.first_mut() {
            first.quantity = 2;
            first.notes = "Com gelo e limão".into();
        }
    });
    render_cart(ui);
    modal(ui, MesasModal::AddOrder);
}
