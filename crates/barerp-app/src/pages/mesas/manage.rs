//! Editar Locais & Mesas (ManageTablesModal): setores, edição em linha, excluir e adicionar mesa.

use std::time::Duration;

use slint::{ComponentHandle, SharedString};

use barerp_db::table_admin::{self, TableInput, DEFAULT_SECTION};
use barerp_db::tables::TableCard;

use super::{modal, refresh, with_state};
use crate::ctx::AppCtx;
use crate::view::model;
use crate::{ManageSection, ManageTable, MesasModal, MesasModalStore};

const NEW_SECTION: &str = "+ Criar Novo Setor...";

fn sections(cards: &[TableCard]) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    for c in cards {
        if !c.section.is_empty() && !list.contains(&c.section) {
            list.push(c.section.clone());
        }
    }
    list
}

/// Redesenha a lista por setor a partir das mesas carregadas na página.
pub(crate) fn render(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    let (groups, options) = with_state(|s| {
        let names = sections(&s.cards);
        let groups: Vec<ManageSection> = names
            .iter()
            .map(|name| ManageSection {
                name: name.clone().into(),
                tables: model(
                    s.cards
                        .iter()
                        .filter(|c| &c.section == name)
                        .map(|c| ManageTable {
                            id: c.id.clone().into(),
                            number: c.number as i32,
                            name: c.name.clone().unwrap_or_default().into(),
                            capacity: c.capacity as i32,
                            section: c.section.clone().into(),
                            free: c.status == "AVAILABLE" && c.order_id.is_none(),
                        })
                        .collect(),
                ),
            })
            .collect();
        let mut options: Vec<SharedString> = names.into_iter().map(SharedString::from).collect();
        if options.is_empty() {
            options.push(DEFAULT_SECTION.into());
        }
        options.push(NEW_SECTION.into());
        (groups, options)
    });
    m.set_g_sections(model(groups));
    m.set_g_section_options(model(options));
}

fn success(ctx: &AppCtx, message: String) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    m.set_busy(false);
    m.set_error(SharedString::new());
    m.set_g_success(message.into());
    let next = ctx.clone();
    slint::Timer::single_shot(Duration::from_secs(3), move || {
        if let Some(ui) = next.ui() {
            ui.global::<MesasModalStore>().set_g_success(SharedString::new());
        }
    });
}

fn failure(ui: &crate::AppWindow, message: String) {
    let m = ui.global::<MesasModalStore>();
    m.set_busy(false);
    m.set_g_success(SharedString::new());
    m.set_error(message.into());
}

/// Setor escolhido no combo (ou o digitado em "Criar Novo Setor").
fn chosen_section(m: &MesasModalStore, value: SharedString) -> Option<String> {
    if value == NEW_SECTION {
        Some(m.get_g_new_custom().trim().to_string()).filter(|s| !s.is_empty())
    } else {
        Some(value.to_string()).filter(|s| !s.is_empty())
    }
}

/// Atualiza as mesas e redesenha a lista.
fn reload(ctx: &AppCtx) {
    refresh(ctx);
    let next = ctx.clone();
    slint::Timer::single_shot(Duration::from_millis(400), move || {
        if let Some(ui) = next.ui() {
            render(&ui);
        }
    });
}

pub(crate) fn open(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    let next_number = with_state(|s| s.cards.iter().map(|c| c.number).max().unwrap_or(0) + 1);
    m.set_g_editing_id(SharedString::new());
    m.set_g_renaming(SharedString::new());
    m.set_g_deleting_id(SharedString::new());
    m.set_g_success(SharedString::new());
    m.set_g_adding(false);
    m.set_g_new_number(next_number.to_string().into());
    m.set_g_new_name(SharedString::new());
    m.set_g_new_capacity(4);
    m.set_g_new_custom(SharedString::new());
    let first = with_state(|s| sections(&s.cards).into_iter().next()).unwrap_or_else(|| DEFAULT_SECTION.to_string());
    m.set_g_new_section(first.into());
    render(&ui);
    modal(&ui, MesasModal::Manage);
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();

    m.on_g_save_edit({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let m = ui.global::<MesasModalStore>();
            let id = m.get_g_editing_id().to_string();
            let Ok(number) = m.get_g_edit_number().trim().parse::<i64>() else {
                failure(&ui, "Informe um número de mesa válido".into());
                return;
            };
            let Some(section) = chosen_section(&m, m.get_g_edit_section()) else {
                failure(&ui, "Digite o nome do novo setor.".into());
                return;
            };
            let name = m.get_g_edit_name().trim().to_string();
            let input = TableInput {
                number,
                name: Some(if name.is_empty() { format!("Mesa {number}") } else { name }),
                capacity: i64::from(m.get_g_edit_capacity()),
                section: Some(section),
            };
            m.set_busy(true);
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { table_admin::update_table(&db, &id, input).await }, move |ui, result| match result {
                Ok(events) => {
                    next.emit(&events);
                    ui.global::<MesasModalStore>().set_g_editing_id(SharedString::new());
                    success(&next, "Mesa atualizada com sucesso!".into());
                    reload(&next);
                }
                Err(e) => failure(ui, e.to_string()),
            });
        }
    });
    m.on_g_confirm_rename({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let m = ui.global::<MesasModalStore>();
            let old = m.get_g_renaming().to_string();
            let new = m.get_g_rename_text().trim().to_string();
            if new.is_empty() || new == old {
                m.set_g_renaming(SharedString::new());
                return;
            }
            m.set_busy(true);
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { table_admin::rename_section(&db, &old, &new).await.map(|r| (r, new)) }, move |ui, result| {
                match result {
                    Ok(((_, events), new)) => {
                        next.emit(&events);
                        ui.global::<MesasModalStore>().set_g_renaming(SharedString::new());
                        success(&next, format!("Local renomeado para \"{new}\"!"));
                        reload(&next);
                    }
                    Err(e) => failure(ui, e.to_string()),
                }
            });
        }
    });
    m.on_g_delete({
        let ctx = ctx.clone();
        move |id| {
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            ctx.spawn(async move { table_admin::delete_table(&db, &id).await }, move |ui, result| {
                ui.global::<MesasModalStore>().set_g_deleting_id(SharedString::new());
                match result {
                    Ok((message, events)) => {
                        next.emit(&events);
                        success(&next, message);
                        reload(&next);
                    }
                    Err(e) => failure(ui, e.to_string()),
                }
            });
        }
    });
    m.on_g_add({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let m = ui.global::<MesasModalStore>();
            let Ok(number) = m.get_g_new_number().trim().parse::<i64>() else {
                failure(&ui, "Informe um número de mesa válido".into());
                return;
            };
            let Some(section) = chosen_section(&m, m.get_g_new_section()) else {
                failure(&ui, "Digite o nome do novo setor.".into());
                return;
            };
            let input = TableInput {
                number,
                name: Some(m.get_g_new_name().to_string()),
                capacity: i64::from(m.get_g_new_capacity()),
                section: Some(section.clone()),
            };
            m.set_busy(true);
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { table_admin::create_table(&db, input).await }, move |ui, result| match result {
                Ok((_, events)) => {
                    next.emit(&events);
                    let m = ui.global::<MesasModalStore>();
                    m.set_g_adding(false);
                    m.set_g_new_number((number + 1).to_string().into());
                    m.set_g_new_name(SharedString::new());
                    success(&next, format!("Mesa {number} criada com sucesso em \"{section}\"!"));
                    reload(&next);
                }
                Err(e) => failure(ui, e.to_string()),
            });
        }
    });
}

/// Gerador de prints: abre a janela com uma mesa em edição.
pub(crate) fn open_preview(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    m.set_g_adding(true);
    m.set_g_new_number("51".into());
    m.set_g_new_section(DEFAULT_SECTION.into());
    m.set_g_renaming(SharedString::new());
    m.set_g_deleting_id(SharedString::new());
    m.set_g_success(SharedString::new());
    let first = with_state(|s| s.cards.first().cloned());
    if let Some(c) = first {
        m.set_g_editing_id(c.id.into());
        m.set_g_edit_number(c.number.to_string().into());
        m.set_g_edit_name(c.name.unwrap_or_default().into());
        m.set_g_edit_capacity(c.capacity as i32);
        m.set_g_edit_section(c.section.into());
    }
    render(ui);
    modal(ui, MesasModal::Manage);
}
