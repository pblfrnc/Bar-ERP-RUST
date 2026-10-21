//! Adicionais e perguntas (`ModifierGroupsManager.tsx`) e promoções do dia (`PromotionsManager.tsx`).

use chrono::Local;
use barerp_core::money::brl;
use barerp_db::modifiers::{self, GroupInput, GroupRow, OptionInput};
use barerp_db::promotions::{self, PromotionInput, PromotionRow};

use super::{ask, emit, keep_number_chars, num_text, parse_num, store, with_state, Confirm};
use crate::ctx::AppCtx;
use crate::view::model;
use crate::{AppWindow, CdChoice, CdModGroup, CdModOption, CdModal, CdPromo};

#[derive(Clone, Default)]
struct OptionDraft {
    id: Option<String>,
    name: String,
    price: String,
    active: bool,
}

#[derive(Default)]
pub(crate) struct ExtrasState {
    // Adicionais
    m_draft: Option<Option<String>>,
    m_options: Vec<OptionDraft>,
    // Promoções
    promos: Vec<PromotionRow>,
    p_draft: Option<Option<String>>,
    p_days: [bool; 7],
    p_categories: Vec<String>,
    p_products: Vec<String>,
}

fn empty_option() -> OptionDraft {
    OptionDraft { id: None, name: String::new(), price: "0".into(), active: true }
}

// ---------- Adicionais ----------

pub(crate) fn render_groups(ui: &AppWindow) {
    let rows: Vec<CdModGroup> = with_state(|s| {
        s.groups
            .iter()
            .map(|g| CdModGroup {
                id: g.id.clone().into(),
                name: g.name.clone().into(),
                rule: g.rule_label().into(),
                required: g.min_select >= 1,
                inactive: !g.is_active,
                options: g.options_label().into(),
                usage: if g.product_count > 0 {
                    format!("Usado em {} produto(s)", g.product_count)
                } else {
                    "Ainda não está em nenhum produto".into()
                }
                .into(),
            })
            .collect()
    });
    store(ui).set_m_groups(model(rows));
}

fn render_options(ui: &AppWindow) {
    let rows: Vec<CdModOption> = with_state(|s| {
        s.extras.m_options.iter().map(|o| CdModOption { name: o.name.clone().into(), price: o.price.clone().into(), active: o.active }).collect()
    });
    store(ui).set_m_options(model(rows));
}

fn reload_groups(ctx: &AppCtx) {
    let db = ctx.db.clone();
    ctx.spawn(async move { modifiers::list(&db).await }, |ui, result| {
        store(ui).set_m_loading(false);
        match result {
            Ok(groups) => {
                with_state(|s| s.groups = groups);
                render_groups(ui);
                super::editor::refresh_derived(ui);
            }
            Err(e) => store(ui).set_modal_error(format!("Erro ao carregar os adicionais: {e}").into()),
        }
    });
}

fn open_draft(ui: &AppWindow, id: Option<String>, name: &str, required: bool, max: &str, active: bool, options: Vec<OptionDraft>) {
    with_state(|s| {
        s.extras.m_draft = Some(id.clone());
        s.extras.m_options = options;
    });
    let st = store(ui);
    st.set_m_draft(true);
    st.set_m_draft_new(id.is_none());
    st.set_m_name(name.into());
    st.set_m_required(required);
    st.set_m_max(max.into());
    st.set_m_active(active);
    st.set_modal_error("".into());
    render_options(ui);
}

fn preset(ui: &AppWindow, which: i32) {
    let opts = |names: &[&str]| names.iter().map(|n| OptionDraft { name: n.to_string(), ..empty_option() }).collect::<Vec<_>>();
    match which {
        0 => open_draft(ui, None, "Ponto da carne", true, "1", true, opts(&["Mal passada", "Ao ponto", "Bem passada"])),
        1 => open_draft(ui, None, "Gelo e limão", false, "2", true, opts(&["Com gelo", "Com limão", "Sem gelo"])),
        _ => open_draft(ui, None, "Adicionais", false, "0", true, vec![empty_option()]),
    }
}

fn edit_group(ui: &AppWindow, g: &GroupRow) {
    let options = g
        .options
        .iter()
        .map(|o| OptionDraft { id: Some(o.id.clone()), name: o.name.clone(), price: num_text(o.price, 2), active: o.is_active })
        .collect();
    open_draft(ui, Some(g.id.clone()), &g.name, g.min_select >= 1, &g.max_select.to_string(), g.is_active, options);
}

fn close_draft(ui: &AppWindow) {
    with_state(|s| s.extras.m_draft = None);
    store(ui).set_m_draft(false);
    store(ui).set_modal_error("".into());
}

fn save_group(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let (id, options) = with_state(|s| (s.extras.m_draft.clone().flatten(), s.extras.m_options.clone()));
    let required = st.get_m_required();
    let options: Vec<OptionInput> = options
        .iter()
        .filter(|o| !o.name.trim().is_empty())
        .map(|o| OptionInput { id: o.id.clone(), name: o.name.trim().into(), price: parse_num(&o.price).unwrap_or(0.0).max(0.0), is_active: o.active })
        .collect();
    // Mesmas mensagens da tela do Node (a regra do banco confere de novo)
    let err = if st.get_m_name().trim().is_empty() {
        Some("Dê um nome ao grupo (ex.: Ponto da carne).")
    } else if options.is_empty() {
        Some("Cadastre pelo menos uma opção.")
    } else if required && !options.iter().any(|o| o.is_active) {
        Some("Uma pergunta obrigatória precisa de pelo menos uma opção ativa.")
    } else {
        None
    };
    if let Some(e) = err {
        st.set_modal_error(e.into());
        return;
    }
    let input = GroupInput {
        name: st.get_m_name().trim().into(),
        min_select: if required { 1 } else { 0 },
        max_select: st.get_m_max().trim().parse::<i64>().unwrap_or(0).max(0),
        is_active: st.get_m_active(),
        options,
    };
    st.set_m_saving(true);
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { modifiers::save(&db, id.as_deref(), input).await }, move |ui, result| {
        store(ui).set_m_saving(false);
        match result {
            Ok((_, events)) => {
                emit(&next, &events);
                close_draft(ui);
                reload_groups(&next);
            }
            Err(e) => store(ui).set_modal_error(e.to_string().into()),
        }
    });
}

pub(crate) fn delete_group(ctx: &AppCtx, id: String) {
    let db = ctx.db.clone();
    let next = ctx.clone();
    let removed = id.clone();
    ctx.spawn(async move { modifiers::delete(&db, &id).await }, move |ui, result| match result {
        Ok(events) => {
            emit(&next, &events);
            // O banco já tirou o grupo dos produtos; o formulário aberto também esquece
            with_state(|s| {
                s.form.group_ids.retain(|g| g != &removed);
                s.form.initial_group_ids.retain(|g| g != &removed);
            });
            reload_groups(&next);
        }
        Err(e) => store(ui).set_modal_error(e.to_string().into()),
    });
}

fn edit_option(ctx: &AppCtx, f: impl FnOnce(&mut Vec<OptionDraft>), rerender: bool) {
    with_state(|s| f(&mut s.extras.m_options));
    if rerender {
        if let Some(ui) = ctx.ui() {
            render_options(&ui);
        }
    }
}

// ---------- Promoções ----------

fn promo_targets(p: &PromotionRow) -> String {
    with_state(|s| {
        let mut names: Vec<String> = p
            .category_id_list()
            .iter()
            .filter_map(|id| s.categories.iter().find(|c| &c.id == id).map(|c| format!("Categoria {}", c.name)))
            .collect();
        names.extend(p.product_id_list().iter().filter_map(|id| s.products.iter().find(|x| &x.id == id).map(|x| x.name.clone())));
        if names.len() > 4 {
            format!("{} e mais {}", names[..4].join(", "), names.len() - 4)
        } else {
            names.join(", ")
        }
    })
}

fn render_promos(ui: &AppWindow) {
    let now = Local::now().naive_local();
    let list = with_state(|s| s.extras.promos.clone());
    let rows: Vec<CdPromo> = list
        .iter()
        .map(|p| {
            let targets = promo_targets(p);
            CdPromo {
                id: p.id.clone().into(),
                name: p.name.clone().into(),
                running: p.is_running_at(now),
                active: p.is_active,
                description: p.describe().into(),
                targets: if targets.is_empty() { "Sem produtos".into() } else { targets.into() },
            }
        })
        .collect();
    store(ui).set_p_list(model(rows));
}

fn reload_promos(ctx: &AppCtx) {
    let db = ctx.db.clone();
    ctx.spawn(async move { promotions::list(&db).await }, |ui, result| {
        store(ui).set_p_loading(false);
        match result {
            Ok(list) => {
                with_state(|s| s.extras.promos = list);
                render_promos(ui);
            }
            Err(e) => store(ui).set_modal_error(format!("Erro ao carregar as promoções: {e}").into()),
        }
    });
}

/// Chips, resultados da busca e prévia do rascunho.
fn render_draft(ui: &AppWindow) {
    let st = store(ui);
    let query = st.get_p_search().trim().to_lowercase();
    let fixed = st.get_p_fixed();
    let value = parse_num(&st.get_p_value()).unwrap_or(0.0);
    let (days, cats, chosen, results, preview, affected) = with_state(|s| {
        let e = &s.extras;
        let cats: Vec<CdChoice> = s
            .categories
            .iter()
            .map(|c| CdChoice { id: c.id.clone().into(), label: c.name.clone().into(), detail: "".into(), selected: e.p_categories.contains(&c.id) })
            .collect();
        let chosen: Vec<CdChoice> = e
            .p_products
            .iter()
            .map(|id| CdChoice {
                id: id.clone().into(),
                label: s.products.iter().find(|p| &p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| "Produto removido".into()).into(),
                detail: "".into(),
                selected: true,
            })
            .collect();
        let results: Vec<CdChoice> = if query.is_empty() {
            Vec::new()
        } else {
            s.products
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&query) || p.code.as_deref().unwrap_or("").to_lowercase().contains(&query))
                .take(30)
                .map(|p| CdChoice { id: p.id.clone().into(), label: p.name.clone().into(), detail: brl(p.price).into(), selected: e.p_products.contains(&p.id) })
                .collect()
        };
        let affected: Vec<_> = s.products.iter().filter(|p| e.p_products.contains(&p.id) || e.p_categories.contains(&p.category_id)).collect();
        let preview: Vec<CdChoice> = affected
            .iter()
            .take(3)
            .map(|p| {
                let to = if fixed { value } else { p.price * (1.0 - value.clamp(0.0, 100.0) / 100.0) };
                let to = (to * 100.0).round() / 100.0;
                let worse = to >= p.price;
                let note = if worse { " (não fica mais barato, a promoção não se aplica)" } else { "" };
                CdChoice { id: p.id.clone().into(), label: format!("{}: {} → {}{note}", p.name, brl(p.price), brl(to)).into(), detail: "".into(), selected: !worse }
            })
            .collect();
        (e.p_days, cats, chosen, results, preview, affected.len())
    });
    st.set_p_days(model(days.to_vec()));
    st.set_p_categories(model(cats));
    st.set_p_chosen(model(chosen));
    st.set_p_results(model(results));
    st.set_p_preview_title(format!("PRÉVIA ({affected} PRODUTO(S) NA PROMOÇÃO)").into());
    st.set_p_preview(model(preview));
}

fn open_promo_draft(ui: &AppWindow, p: Option<&PromotionRow>) {
    let st = store(ui);
    with_state(|s| {
        let e = &mut s.extras;
        e.p_draft = Some(p.map(|p| p.id.clone()));
        e.p_days = [false; 7];
        match p {
            Some(p) => {
                for d in p.days() {
                    e.p_days[d as usize] = true;
                }
                e.p_categories = p.category_id_list();
                e.p_products = p.product_id_list();
            }
            None => {
                e.p_days = [true; 7];
                e.p_categories.clear();
                e.p_products.clear();
            }
        }
    });
    st.set_p_draft(true);
    st.set_p_draft_new(p.is_none());
    st.set_p_name(p.map(|p| p.name.clone()).unwrap_or_default().into());
    st.set_p_active(p.is_none_or(|p| p.is_active));
    st.set_p_start(p.map(|p| p.start_time.clone()).unwrap_or_else(|| "17:00".into()).into());
    st.set_p_end(p.map(|p| p.end_time.clone()).unwrap_or_else(|| "20:00".into()).into());
    st.set_p_fixed(p.is_some_and(|p| p.discount_type == "FIXED_PRICE"));
    st.set_p_value(p.map(|p| num_text(p.value, 2)).unwrap_or_default().into());
    st.set_p_search("".into());
    st.set_modal_error("".into());
    render_draft(ui);
}

fn close_promo_draft(ui: &AppWindow) {
    with_state(|s| s.extras.p_draft = None);
    store(ui).set_p_draft(false);
    store(ui).set_modal_error("".into());
}

fn save_promo(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let (id, days, cats, prods) = with_state(|s| {
        let e = &s.extras;
        (e.p_draft.clone().flatten(), e.p_days, e.p_categories.clone(), e.p_products.clone())
    });
    let value = parse_num(&st.get_p_value());
    let fixed = st.get_p_fixed();
    let err = if st.get_p_name().trim().is_empty() {
        Some("Dê um nome à promoção (ex.: Happy Hour do Chopp).")
    } else if value.is_none_or(|v| v <= 0.0) {
        Some("Informe o valor do desconto.")
    } else if !fixed && value.unwrap_or(0.0) >= 100.0 {
        Some("O desconto em % precisa ser menor que 100.")
    } else if !days.iter().any(|d| *d) {
        Some("Escolha pelo menos um dia da semana.")
    } else if st.get_p_start().trim().is_empty() || st.get_p_end().trim().is_empty() {
        Some("Informe o horário de início e fim.")
    } else if cats.is_empty() && prods.is_empty() {
        Some("Escolha os produtos ou categorias da promoção.")
    } else {
        None
    };
    if let Some(e) = err {
        st.set_modal_error(e.into());
        return;
    }
    let input = PromotionInput {
        name: st.get_p_name().trim().into(),
        is_active: st.get_p_active(),
        days: (0..7u32).filter(|d| days[*d as usize]).collect(),
        start_time: st.get_p_start().trim().into(),
        end_time: st.get_p_end().trim().into(),
        fixed_price: fixed,
        value: value.unwrap_or(0.0),
        product_ids: prods,
        category_ids: cats,
    };
    st.set_p_saving(true);
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { promotions::save(&db, id.as_deref(), input).await }, move |ui, result| {
        store(ui).set_p_saving(false);
        match result {
            Ok((_, events)) => {
                emit(&next, &events);
                close_promo_draft(ui);
                reload_promos(&next);
            }
            Err(e) => store(ui).set_modal_error(e.to_string().into()),
        }
    });
}

pub(crate) fn delete_promotion(ctx: &AppCtx, id: String) {
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { promotions::delete(&db, &id).await }, move |ui, result| match result {
        Ok(events) => {
            emit(&next, &events);
            reload_promos(&next);
        }
        Err(e) => store(ui).set_modal_error(e.to_string().into()),
    });
}

fn toggle_in(list: &mut Vec<String>, id: &str) {
    if let Some(i) = list.iter().position(|x| x == id) {
        list.remove(i);
    } else {
        list.push(id.to_string());
    }
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);

    // ----- Adicionais -----
    st.on_open_modifiers({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            close_draft(&ui);
            store(&ui).set_m_loading(true);
            render_groups(&ui);
            store(&ui).set_modal(CdModal::Modifiers);
            reload_groups(&ctx);
        }
    });
    st.on_m_new({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                open_draft(&ui, None, "", false, "0", true, vec![empty_option()]);
            }
        }
    });
    st.on_m_preset({
        let ctx = ctx.clone();
        move |i| {
            if let Some(ui) = ctx.ui() {
                preset(&ui, i);
            }
        }
    });
    st.on_m_edit({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            if let Some(g) = with_state(|s| s.groups.iter().find(|g| g.id == id.as_str()).cloned()) {
                edit_group(&ui, &g);
            }
        }
    });
    st.on_m_delete({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let Some(g) = with_state(|s| s.groups.iter().find(|g| g.id == id.as_str()).cloned()) else { return };
            let used = if g.product_count > 0 { format!("\n\nEle está em {} produto(s) e sairá deles.", g.product_count) } else { String::new() };
            ask(&ui, "Excluir grupo", &format!(r#"Excluir o grupo "{}"?{used}"#, g.name), "Excluir", true, Confirm::DeleteGroup(g.id.clone()));
        }
    });
    st.on_m_cancel({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                close_draft(&ui);
            }
        }
    });
    st.on_m_save({
        let ctx = ctx.clone();
        move || save_group(&ctx)
    });
    st.on_m_opt_add({
        let ctx = ctx.clone();
        move || edit_option(&ctx, |o| o.push(empty_option()), true)
    });
    st.on_m_opt_remove({
        let ctx = ctx.clone();
        move |i| {
            edit_option(
                &ctx,
                |o| {
                    if (i as usize) < o.len() {
                        o.remove(i as usize);
                    }
                },
                true,
            )
        }
    });
    st.on_m_opt_move({
        let ctx = ctx.clone();
        move |i, delta| {
            edit_option(
                &ctx,
                |o| {
                    let target = i + delta;
                    if i >= 0 && target >= 0 && (target as usize) < o.len() && (i as usize) < o.len() {
                        o.swap(i as usize, target as usize);
                    }
                },
                true,
            )
        }
    });
    st.on_m_opt_name({
        let ctx = ctx.clone();
        move |i, text| {
            edit_option(
                &ctx,
                |o| {
                    if let Some(x) = o.get_mut(i as usize) {
                        x.name = text.to_string();
                    }
                },
                false,
            )
        }
    });
    st.on_m_opt_price({
        let ctx = ctx.clone();
        move |i, text| {
            edit_option(
                &ctx,
                |o| {
                    if let Some(x) = o.get_mut(i as usize) {
                        x.price = keep_number_chars(&text);
                    }
                },
                false,
            )
        }
    });
    st.on_m_opt_active({
        let ctx = ctx.clone();
        move |i, active| {
            edit_option(
                &ctx,
                |o| {
                    if let Some(x) = o.get_mut(i as usize) {
                        x.active = active;
                    }
                },
                true,
            )
        }
    });

    // ----- Promoções -----
    st.on_open_promotions({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            close_promo_draft(&ui);
            store(&ui).set_p_loading(true);
            store(&ui).set_modal(CdModal::Promotions);
            reload_promos(&ctx);
        }
    });
    st.on_p_new({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                open_promo_draft(&ui, None);
            }
        }
    });
    st.on_p_edit({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            if let Some(p) = with_state(|s| s.extras.promos.iter().find(|p| p.id == id.as_str()).cloned()) {
                open_promo_draft(&ui, Some(&p));
            }
        }
    });
    st.on_p_delete({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let name = with_state(|s| s.extras.promos.iter().find(|p| p.id == id.as_str()).map(|p| p.name.clone())).unwrap_or_default();
            ask(&ui, "Excluir promoção", &format!(r#"Excluir a promoção "{name}"?"#), "Excluir", true, Confirm::DeletePromotion(id.to_string()));
        }
    });
    st.on_p_toggle({
        let ctx = ctx.clone();
        move |id| {
            let Some(active) = with_state(|s| s.extras.promos.iter().find(|p| p.id == id.as_str()).map(|p| p.is_active)) else { return };
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            ctx.spawn(async move { promotions::set_active(&db, &id, !active).await }, move |ui, result| match result {
                Ok(events) => {
                    emit(&next, &events);
                    reload_promos(&next);
                }
                Err(e) => store(ui).set_modal_error(format!("Erro ao atualizar a promoção: {e}").into()),
            });
        }
    });
    st.on_p_cancel({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                close_promo_draft(&ui);
            }
        }
    });
    st.on_p_save({
        let ctx = ctx.clone();
        move || save_promo(&ctx)
    });
    let rerender = |ctx: &AppCtx| {
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render_draft(&ui);
            }
        }
    };
    st.on_p_search_changed(rerender(ctx));
    st.on_p_day({
        let ctx = ctx.clone();
        move |day| {
            with_state(|s| {
                if let Some(d) = s.extras.p_days.get_mut(day as usize) {
                    *d = !*d;
                }
            });
            if let Some(ui) = ctx.ui() {
                render_draft(&ui);
            }
        }
    });
    st.on_p_all_days({
        let ctx = ctx.clone();
        move || {
            with_state(|s| s.extras.p_days = [true; 7]);
            if let Some(ui) = ctx.ui() {
                render_draft(&ui);
            }
        }
    });
    st.on_p_weekend({
        let ctx = ctx.clone();
        move || {
            // Sexta, sábado e domingo
            with_state(|s| s.extras.p_days = [true, false, false, false, false, true, true]);
            if let Some(ui) = ctx.ui() {
                render_draft(&ui);
            }
        }
    });
    st.on_p_cat({
        let ctx = ctx.clone();
        move |id| {
            with_state(|s| toggle_in(&mut s.extras.p_categories, &id));
            if let Some(ui) = ctx.ui() {
                render_draft(&ui);
            }
        }
    });
    st.on_p_prod({
        let ctx = ctx.clone();
        move |id| {
            with_state(|s| toggle_in(&mut s.extras.p_products, &id));
            if let Some(ui) = ctx.ui() {
                render_draft(&ui);
            }
        }
    });
    st.on_p_type({
        let ctx = ctx.clone();
        move |fixed| {
            if let Some(ui) = ctx.ui() {
                store(&ui).set_p_fixed(fixed);
                render_draft(&ui);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alterna_ids() {
        let mut v = vec!["a".to_string()];
        toggle_in(&mut v, "b");
        toggle_in(&mut v, "a");
        assert_eq!(v, vec!["b"]);
    }
}
