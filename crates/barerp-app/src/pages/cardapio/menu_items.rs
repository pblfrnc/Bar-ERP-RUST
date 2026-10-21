//! Itens do cardápio digital (`DigitalMenuItemsModal.tsx`): marcar o que aparece para o cliente, com foto e
//! descrição. Comidas antes das bebidas, subgrupos automáticos pelo nome, aviso de estoque da loja.

use std::collections::{HashMap, HashSet};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use barerp_catalog::menu_groups::{group_rank, menu_group_of, NOT_FOR_MENU_GROUPS};
use barerp_catalog::photo;
use barerp_core::money::brl;
use barerp_db::events::Event;
use barerp_db::products::{self, ProductRow};
use barerp_db::{digital_menu, DbResult};
use barerp_menu_publish::db_store::save_product_image;

use super::editor::choose_photo_file;
use super::{ask, close_modal, emit, reload, store, with_state, Confirm};
use crate::ctx::AppCtx;
use crate::view::model;
use crate::{AppWindow, CdMenuRow, CdModal};

#[derive(Default)]
pub(crate) struct MenuState {
    selected: HashSet<String>,
    initial: HashSet<String>,
    hidden_categories: Vec<String>,
    expanded: Option<String>,
    desc_original: String,
    /// Miniaturas já abertas: productId → (hash, imagem)
    thumbs: HashMap<String, (String, Image)>,
}

/// Sem acento e minúsculo (busca do original).
fn normalize(s: &str) -> String {
    barerp_catalog::menu_groups::plain_name(s).to_lowercase()
}

fn changes() -> (Vec<String>, Vec<String>) {
    with_state(|s| {
        let m = &s.menu;
        let show = m.selected.difference(&m.initial).cloned().collect();
        let hide = m.initial.difference(&m.selected).cloned().collect();
        (show, hide)
    })
}

/// Seções da tela: (categoria, é comida?, produtos visíveis, subgrupos (nome, produtos)).
type Section = (String, String, bool, Vec<ProductRow>, Vec<(Option<&'static str>, Vec<ProductRow>)>);

fn sections(query: &str, filter: i32) -> Vec<Section> {
    with_state(|s| {
        let q = normalize(query.trim());
        let visible: Vec<&ProductRow> = s
            .products
            .iter()
            .filter(|p| {
                let text = normalize(&format!("{} {} {}", p.name, p.code.as_deref().unwrap_or(""), p.description.as_deref().unwrap_or("")));
                (q.is_empty() || text.contains(&q))
                    && match filter {
                        1 => s.menu.selected.contains(&p.id),
                        2 => !s.menu.selected.contains(&p.id),
                        3 => p.image_hash.is_none(),
                        _ => true,
                    }
            })
            .collect();
        let mut cats = s.categories.clone();
        cats.sort_by_key(|c| c.sort_order);
        cats.iter()
            .filter_map(|c| {
                let all: Vec<&ProductRow> = s.products.iter().filter(|p| p.category_id == c.id).collect();
                let kitchen = all.iter().filter(|p| p.kds_station == "KITCHEN").count();
                let list: Vec<ProductRow> = visible.iter().filter(|p| p.category_id == c.id).map(|p| (*p).clone()).collect();
                if list.is_empty() {
                    return None;
                }
                let mut buckets: Vec<(&'static str, Vec<ProductRow>)> = Vec::new();
                for p in &list {
                    let g = menu_group_of(&p.name);
                    match buckets.iter_mut().find(|(n, _)| *n == g) {
                        Some((_, v)) => v.push(p.clone()),
                        None => buckets.push((g, vec![p.clone()])),
                    }
                }
                let subgroups = if buckets.len() > 1 {
                    buckets.sort_by_key(|(g, _)| group_rank(g));
                    buckets.into_iter().map(|(g, v)| (Some(g), v)).collect()
                } else {
                    vec![(None, list.clone())]
                };
                Some((c.id.clone(), c.name.clone(), kitchen * 2 > all.len(), list, subgroups))
            })
            .collect()
    })
}

fn row(kind: i32, id: String, title: String) -> CdMenuRow {
    CdMenuRow { kind, id: id.into(), title: title.into(), ..Default::default() }
}

pub(crate) fn render(ui: &AppWindow) {
    let st = store(ui);
    let secs = sections(&st.get_d_search(), st.get_d_filter());
    let mut rows: Vec<CdMenuRow> = Vec::new();
    with_state(|s| {
        let m = &s.menu;
        for (food, label, id) in [(true, "COMIDAS", "food"), (false, "BEBIDAS", "drink")] {
            let group: Vec<&Section> = secs.iter().filter(|x| x.2 == food).collect();
            if group.is_empty() {
                continue;
            }
            rows.push(row(0, id.into(), label.into()));
            for (cat_id, cat_name, _, list, subgroups) in group {
                let all_on = list.iter().all(|p| m.selected.contains(&p.id));
                let mut head = row(1, cat_id.clone(), cat_name.clone());
                head.action = if all_on { "Desmarcar todos" } else { "Marcar todos" }.into();
                if m.hidden_categories.contains(cat_id) {
                    head.subtitle = "Categoria escondida nas configurações do cardápio: estes itens não aparecem.".into();
                }
                rows.push(head);
                for (name, items) in subgroups {
                    if let Some(g) = name {
                        let on = items.iter().filter(|p| m.selected.contains(&p.id)).count();
                        let mut sub = row(2, format!("{cat_id}|{g}"), g.to_uppercase());
                        sub.subtitle = format!("({on} de {} no cardápio)", items.len()).into();
                        sub.action = if on < items.len() { "Marcar todos" } else { "Desmarcar todos" }.into();
                        rows.push(sub);
                    }
                    for p in items {
                        let mut r = row(3, p.id.clone(), p.name.clone());
                        r.selected = m.selected.contains(&p.id);
                        r.no_price = p.price <= 0.0;
                        r.subtitle = if r.no_price {
                            "Sem preço de venda: não aparece no cardápio".to_string()
                        } else {
                            match p.description.as_deref().filter(|d| !d.is_empty()) {
                                Some(d) => format!("{} · {d}", brl(p.price)),
                                None => brl(p.price),
                            }
                        }
                        .into();
                        if let Some((_, img)) = m.thumbs.get(&p.id).filter(|(h, _)| Some(h) == p.image_hash.as_ref()) {
                            r.has_photo = true;
                            r.photo = img.clone();
                        }
                        r.expanded = m.expanded.as_deref() == Some(p.id.as_str());
                        rows.push(r);
                    }
                }
            }
        }
        let not_menu: Vec<&ProductRow> = s
            .products
            .iter()
            .filter(|p| m.selected.contains(&p.id) && NOT_FOR_MENU_GROUPS.contains(&menu_group_of(&p.name)))
            .collect();
        st.set_d_not_menu_count(not_menu.len() as i32);
        st.set_d_not_menu(
            format!(
                "{} itens parecem ser estoque da loja ({}) e estão no cardápio. Confira e tire do cardápio de uma vez; depois clique em Salvar.",
                not_menu.len(),
                NOT_FOR_MENU_GROUPS.join(", ")
            )
            .into(),
        );
        st.set_d_count(format!("{} de {} no cardápio", m.selected.len(), s.products.len()).into());
    });
    let (show, hide) = changes();
    st.set_d_changes((show.len() + hide.len()) as i32);
    st.set_d_rows(model(rows));
}

/// Abre as miniaturas das fotos que ainda não estão em memória.
fn load_thumbs(ctx: &AppCtx) {
    let wanted: Vec<(String, String)> = with_state(|s| {
        s.products
            .iter()
            .filter_map(|p| {
                let h = p.image_hash.clone()?;
                (s.menu.thumbs.get(&p.id).map(|(x, _)| x) != Some(&h)).then(|| (p.id.clone(), h))
            })
            .collect()
    });
    if wanted.is_empty() {
        return;
    }
    let db = ctx.db.clone();
    ctx.spawn(
        async move {
            let mut out = Vec::new();
            for (id, hash) in wanted {
                if let Ok(Some(img)) = digital_menu::product_image(&db, &id).await {
                    if let Some(px) = photo::thumbnail_rgba(&img.data, 96) {
                        out.push((id, hash, px));
                    }
                }
            }
            out
        },
        |ui, thumbs| {
            with_state(|s| {
                for (id, hash, (w, h, px)) in thumbs {
                    let image = Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, w, h));
                    s.menu.thumbs.insert(id, (hash, image));
                }
            });
            render(ui);
        },
    );
}

/// Marca como selecionados os produtos que já estão no cardápio (abertura da tela).
pub(crate) fn start(ui: &AppWindow) {
    with_state(|s| {
        let sel: HashSet<String> = s.products.iter().filter(|p| p.show_in_menu).map(|p| p.id.clone()).collect();
        s.menu.initial = sel.clone();
        s.menu.selected = sel;
        s.menu.expanded = None;
    });
    let st = store(ui);
    st.set_d_search("".into());
    st.set_d_filter(0);
    st.set_d_feedback("".into());
    st.set_d_loading(false);
    st.set_d_saving(false);
    st.set_modal(CdModal::MenuItems);
    render(ui);
}

fn open(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    start(&ui);
    load_thumbs(ctx);
    let db = ctx.db.clone();
    ctx.spawn(async move { digital_menu::settings(&db).await }, |ui, result| {
        if let Ok(row) = result {
            let hidden: Vec<String> = serde_json::from_str(&row.hidden_category_ids).unwrap_or_default();
            with_state(|s| s.menu.hidden_categories = hidden);
            render(ui);
        }
    });
}

/// Fecha (depois da confirmação, se havia alteração) e recarrega a lista de produtos.
pub(crate) fn close(ctx: &AppCtx) {
    if let Some(ui) = ctx.ui() {
        close_modal(&ui);
    }
    reload(ctx);
}

fn set_many(ids: Vec<String>, on: bool) {
    with_state(|s| {
        for id in ids {
            if on {
                s.menu.selected.insert(id);
            } else {
                s.menu.selected.remove(&id);
            }
        }
    });
}

fn feedback(ui: &AppWindow, text: impl Into<slint::SharedString>, error: bool) {
    store(ui).set_d_feedback(text.into());
    store(ui).set_d_error(error);
}

fn set_hash(id: &str, hash: Option<String>) {
    with_state(|s| {
        if let Some(p) = s.products.iter_mut().find(|p| p.id == id) {
            p.image_hash = hash;
        }
    });
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let rerender = |ctx: &AppCtx| {
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render(&ui);
            }
        }
    };
    st.on_open_menu_items({
        let ctx = ctx.clone();
        move || open(&ctx)
    });
    st.on_d_search_changed(rerender(ctx));
    st.on_d_filter_pick({
        let ctx = ctx.clone();
        move |i| {
            if let Some(ui) = ctx.ui() {
                store(&ui).set_d_filter(i);
                render(&ui);
            }
        }
    });
    st.on_d_toggle({
        let ctx = ctx.clone();
        move |id| {
            with_state(|s| {
                let id = id.to_string();
                if !s.menu.selected.remove(&id) {
                    s.menu.selected.insert(id);
                }
            });
            if let Some(ui) = ctx.ui() {
                render(&ui);
            }
        }
    });
    st.on_d_toggle_many({
        let ctx = ctx.clone();
        move |kind, key| {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let secs = sections(&st.get_d_search(), st.get_d_filter());
            let (cat, group) = match key.split_once('|') {
                Some((c, g)) => (c.to_string(), Some(g.to_string())),
                None => (key.to_string(), None),
            };
            let ids: Vec<String> = secs
                .iter()
                .filter(|x| x.0 == cat)
                .flat_map(|x| {
                    if kind == 2 {
                        x.4.iter().filter(|(n, _)| n.map(str::to_string) == group).flat_map(|(_, v)| v.iter().map(|p| p.id.clone())).collect::<Vec<_>>()
                    } else {
                        x.3.iter().map(|p| p.id.clone()).collect()
                    }
                })
                .collect();
            let on_count = with_state(|s| ids.iter().filter(|i| s.menu.selected.contains(*i)).count());
            set_many(ids.clone(), on_count < ids.len());
            render(&ui);
        }
    });
    st.on_d_uncheck_store({
        let ctx = ctx.clone();
        move || {
            let ids = with_state(|s| {
                s.products
                    .iter()
                    .filter(|p| s.menu.selected.contains(&p.id) && NOT_FOR_MENU_GROUPS.contains(&menu_group_of(&p.name)))
                    .map(|p| p.id.clone())
                    .collect()
            });
            set_many(ids, false);
            if let Some(ui) = ctx.ui() {
                render(&ui);
            }
        }
    });
    st.on_d_expand({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let desc = with_state(|s| {
                if s.menu.expanded.as_deref() == Some(id.as_str()) {
                    s.menu.expanded = None;
                    return None;
                }
                s.menu.expanded = Some(id.to_string());
                let d = s.products.iter().find(|p| p.id == id.as_str()).and_then(|p| p.description.clone()).unwrap_or_default();
                s.menu.desc_original = d.clone();
                Some(d)
            });
            if let Some(d) = desc {
                store(&ui).set_d_desc(d.into());
                store(&ui).set_d_desc_changed(false);
            }
            feedback(&ui, "", false);
            render(&ui);
        }
    });
    st.on_d_desc_edited({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let orig = with_state(|s| s.menu.desc_original.clone());
            store(&ui).set_d_desc_changed(store(&ui).get_d_desc().trim() != orig.trim());
        }
    });
    st.on_d_save_desc({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let desc: String = store(&ui).get_d_desc().chars().take(500).collect();
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            let (pid, d) = (id.clone(), desc.clone());
            ctx.spawn(async move { products::set_description(&db, &id, Some(&desc)).await }, move |ui, result| match result {
                Ok(events) => {
                    emit(&next, &events);
                    let trimmed = d.trim().to_string();
                    let name = with_state(|s| {
                        s.menu.desc_original = trimmed.clone();
                        let p = s.products.iter_mut().find(|p| p.id == pid)?;
                        p.description = Some(trimmed.clone()).filter(|t| !t.is_empty());
                        Some(p.name.clone())
                    });
                    store(ui).set_d_desc_changed(false);
                    feedback(ui, format!(r#"Descrição de "{}" salva."#, name.unwrap_or_default()), false);
                    render(ui);
                }
                Err(e) => feedback(ui, format!("Erro ao salvar a descrição: {e}"), true),
            });
        }
    });
    st.on_d_photo({
        let ctx = ctx.clone();
        move |id| {
            let Some(data) = choose_photo_file() else { return };
            let Some(ui) = ctx.ui() else { return };
            store(&ui).set_d_photo_busy(id.clone());
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            let pid = id.clone();
            ctx.spawn(
                async move {
                    let jpeg = photo::shrink_photo(&data).map_err(barerp_db::DbError::rule)?;
                    let hash = save_product_image(&db, &id, jpeg).await?;
                    DbResult::Ok(hash)
                },
                move |ui, result| {
                    store(ui).set_d_photo_busy("".into());
                    match result {
                        Ok(hash) => {
                            emit(&next, &[Event::CatalogUpdated { scope: "images" }]);
                            set_hash(&pid, Some(hash));
                            load_thumbs(&next);
                        }
                        Err(e) => feedback(ui, format!("Erro ao enviar a foto: {e}"), true),
                    }
                },
            );
        }
    });
    st.on_d_remove_photo({
        let ctx = ctx.clone();
        move |id| {
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            let pid = id.clone();
            ctx.spawn(async move { digital_menu::delete_product_image(&db, &id).await }, move |ui, result| match result {
                Ok(()) => {
                    emit(&next, &[Event::CatalogUpdated { scope: "images" }]);
                    set_hash(&pid, None);
                    with_state(|s| s.menu.thumbs.remove(&pid));
                    render(ui);
                }
                Err(e) => feedback(ui, format!("Erro ao remover a foto: {e}"), true),
            });
        }
    });
    st.on_d_save({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let (show, hide) = changes();
            store(&ui).set_d_saving(true);
            feedback(&ui, "", false);
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { digital_menu::set_menu_visibility(&db, &show, &hide).await }, move |ui, result| {
                store(ui).set_d_saving(false);
                match result {
                    Ok(()) => {
                        emit(&next, &[Event::CatalogUpdated { scope: "products" }]);
                        with_state(|s| {
                            s.menu.initial = s.menu.selected.clone();
                            for p in s.products.iter_mut() {
                                p.show_in_menu = s.menu.selected.contains(&p.id);
                            }
                        });
                        feedback(ui, "Itens salvos. O cardápio digital é atualizado em alguns segundos.", false);
                        render(ui);
                    }
                    Err(e) => feedback(ui, format!("Erro ao salvar os itens do cardápio: {e}"), true),
                }
            });
        }
    });
    st.on_d_close({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let (show, hide) = changes();
            if show.len() + hide.len() > 0 {
                ask(
                    &ui,
                    "Sair sem salvar?",
                    "Há itens marcados/desmarcados que não foram salvos. Sair mesmo assim?",
                    "Sair",
                    true,
                    Confirm::CloseMenuItems,
                );
            } else {
                close(&ctx);
            }
        }
    });
}
