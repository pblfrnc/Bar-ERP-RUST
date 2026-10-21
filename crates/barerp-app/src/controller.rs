//! Casca do app: login, troca de senha, sair, tema e escala de fonte, status do servidor dos celulares e os
//! comprovantes que os celulares mandam imprimir no caixa. As telas de cada área ficam em `src/pages/*.rs`.
//!
//! As consultas rodam no runtime do tokio e o resultado volta para a thread da interface por
//! `upgrade_in_event_loop`. O usuário logado só existe na thread da interface.

use std::cell::RefCell;
use std::time::Duration;

use slint::{ComponentHandle, SharedString, Timer, TimerMode};
use tokio::runtime::Handle;

use barerp_db::staff::{self, LoggedUser};
use barerp_db::Db;
use barerp_server::ServerHandle;

use crate::ctx::AppCtx;
use crate::view::model;
use crate::{AppWindow, CaixaStore, Page, StaffItem};

thread_local! {
    static USER: RefCell<Option<LoggedUser>> = const { RefCell::new(None) };
    static LAYOUT_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// Nome do usuário logado (usado pelas páginas para auditoria e "aberto por").
pub fn logged_user_name() -> Option<String> {
    USER.with(|u| u.borrow().as_ref().map(|u| u.name.clone()))
}

fn status(ui: &AppWindow, message: impl Into<SharedString>, error: bool) {
    ui.set_status_message(message.into());
    ui.set_status_error(error);
}

fn load_staff(ctx: &AppCtx) {
    let db = ctx.db.clone();
    ctx.spawn(async move { staff::public_list(&db).await }, |ui, result| match result {
        Ok(list) => {
            let items = list
                .into_iter()
                .map(|s| StaffItem { id: s.id.into(), name: s.name.into(), role: s.role.into() })
                .collect();
            ui.set_staff(model(items));
        }
        Err(e) => ui.set_login_error(format!("Não foi possível ler os funcionários: {e}").into()),
    });
}

/// Liga a casca do app e todas as páginas. O `Timer` devolvido precisa viver enquanto a janela estiver aberta.
pub fn start(ui: &AppWindow, db: Db, rt: Handle, db_label: String, server: Option<ServerHandle>) -> Timer {
    let ctx = AppCtx::new(ui, db, rt, server);
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    // Abre em tela cheia (app.slint); ao sair dela com F11 a janela fica maximizada, não no tamanho padrão
    ui.window().set_maximized(true);
    status(ui, format!("Banco: {db_label}"), false);

    // Tema e escala de fonte lembrados neste PC
    // (relidas do arquivo a cada mudança: a Retaguarda grava a auto-impressão no mesmo arquivo)
    crate::ctx::apply_preferences(ui, crate::ctx::Preferences::load());
    ui.on_toggle_theme({
        let weak = ui.as_weak();
        move || {
            let mut p = crate::ctx::Preferences::load();
            p.dark = !p.dark;
            p.save();
            if let Some(ui) = weak.upgrade() {
                crate::ctx::apply_preferences(&ui, p);
            }
        }
    });
    ui.on_cycle_font_scale({
        let weak = ui.as_weak();
        move || {
            let mut p = crate::ctx::Preferences::load();
            p.font_scale = p.font_scale.next();
            p.save();
            if let Some(ui) = weak.upgrade() {
                crate::ctx::apply_preferences(&ui, p);
            }
        }
    });

    // Páginas por área (src/pages/*.rs)
    crate::pages::bind_all(&ctx);

    // Recibos que os celulares (garçons) pedem para imprimir na impressora do caixa
    if let Some(srv) = &ctx.server {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<barerp_server::PrintRequest>(16);
        srv.set_local_printer(tx);
        let print_db = ctx.db.clone();
        let ui_weak = ui.as_weak();
        ctx.rt.spawn(async move {
            while let Some(req) = rx.recv().await {
                let order_id = req.order.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                let requested_by = req.requested_by.clone();
                let ui_weak = ui_weak.clone();
                let db = print_db.clone();
                tokio::spawn(async move {
                    let res = match order_id {
                        Some(id) => crate::print::print_order_receipt(&db, &id).await.map(|_| ()),
                        None => Err("ID da comanda ausente no pedido de impressão.".to_string()),
                    };
                    match res {
                        Ok(()) => {
                            let _ = req.reply.send(Ok(()));
                            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                let text = match requested_by {
                                    Some(by) => format!("Comprovante impresso (pedido por {by})"),
                                    None => "Comprovante impresso com sucesso.".to_string(),
                                };
                                status(&ui, text, false);
                            });
                        }
                        Err(err) => {
                            let _ = req.reply.send(Err(err.clone()));
                            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                status(&ui, format!("Falha na impressão do celular: {err}"), true);
                            });
                        }
                    }
                });
            }
        });
    }

    load_staff(&ctx);

    // ----- Login, senha, sair -----
    ui.on_login({
        let ctx = ctx.clone();
        move |user_id, pin| {
            let Some(ui) = ctx.ui() else { return };
            ui.set_login_busy(true);
            ui.set_login_error(SharedString::new());
            let db = ctx.db.clone();
            ctx.spawn(async move { staff::login(&db, &user_id, &pin).await }, move |ui, result| {
                ui.set_login_busy(false);
                match result {
                    Ok(user) => {
                        ui.set_user_name(user.name.clone().into());
                        ui.set_user_role(user.role.clone().into());
                        // Abas visíveis conforme as permissões (ADMIN vê tudo), como no Navbar original
                        let allowed = |id: &str| user.role == "ADMIN" || user.permissions.iter().any(|p| p == id);
                        let nav = crate::NavPerms {
                            tables: allowed("tables"),
                            kds: allowed("kds"),
                            cash: allowed("cash"),
                            products: allowed("products"),
                            fiscal: allowed("fiscal"),
                            settings: allowed("settings"),
                        };
                        ui.set_nav(nav.clone());
                        if user.default_password {
                            ui.set_password_hint(
                                "Você entrou com a senha padrão 1234. Troque agora para ninguém mais entrar com o seu usuário."
                                    .into(),
                            );
                            ui.set_password_error(SharedString::new());
                            ui.set_password_dialog_open(true);
                        }
                        // "Operador / Responsável" do caixa já vem com quem entrou
                        ui.global::<crate::CardapioStore>().set_is_admin(user.role == "ADMIN");
                        let caixa = ui.global::<CaixaStore>();
                        caixa.set_open_by(user.name.clone().into());
                        caixa.set_close_by(user.name.clone().into());
                        USER.with(|u| *u.borrow_mut() = Some(user));
                        // Primeira aba permitida (o original abre em Mesas)
                        let first = [
                            (nav.tables, Page::Tables),
                            (nav.kds, Page::Kds),
                            (nav.cash, Page::Cash),
                            (nav.products, Page::Products),
                            (nav.fiscal, Page::Fiscal),
                            (nav.settings, Page::Settings),
                        ]
                        .into_iter()
                        .find_map(|(ok, page)| ok.then_some(page))
                        .unwrap_or(Page::Tables);
                        ui.set_page(first);
                    }
                    Err(e) => ui.set_login_error(e.to_string().into()),
                }
            });
        }
    });

    ui.on_change_password({
        let ctx = ctx.clone();
        move |current, new, confirm| {
            let Some(user_id) = USER.with(|u| u.borrow().as_ref().map(|u| u.id.clone())) else { return };
            if let Some(ui) = ctx.ui() {
                ui.set_password_busy(true);
                ui.set_password_error(SharedString::new());
            }
            let db = ctx.db.clone();
            ctx.spawn(
                async move { staff::change_password(&db, &user_id, &current, &new, &confirm).await },
                |ui, result| {
                    ui.set_password_busy(false);
                    match result {
                        Ok(()) => {
                            ui.set_password_dialog_open(false);
                            ui.set_password_hint(SharedString::new());
                            status(ui, "Senha trocada com sucesso.", false);
                        }
                        Err(e) => ui.set_password_error(e.to_string().into()),
                    }
                },
            );
        }
    });

    ui.on_password_dialog_closed({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                ui.set_password_hint(SharedString::new());
                ui.set_password_error(SharedString::new());
            }
        }
    });

    ui.on_logout({
        let ctx = ctx.clone();
        move || {
            USER.with(|u| *u.borrow_mut() = None);
            if let Some(ui) = ctx.ui() {
                ui.set_user_name(SharedString::new());
                ui.set_user_role(SharedString::new());
                ui.set_password_dialog_open(false);
                ui.set_page(Page::Login);
            }
            load_staff(&ctx);
        }
    });

    // Zoom da fonte e barra superior compacta conforme a largura real da janela (verificados 4 vezes por segundo)
    let layout_timer = Timer::default();
    layout_timer.start(TimerMode::Repeated, Duration::from_millis(250), {
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            crate::ctx::sync_zoom(&ui);
            let width = ui.window().size().width as f32 / ui.window().scale_factor();
            let (compact, tiny) = (width < 1500.0, width < 1180.0);
            if ui.get_compact() != compact {
                ui.set_compact(compact);
            }
            if ui.get_tiny() != tiny {
                ui.set_tiny(tiny);
            }
        }
    });
    LAYOUT_TIMER.with(|t| *t.borrow_mut() = Some(layout_timer));

    // Servidor dos celulares: Online/Offline na barra e o motivo no rodapé (ex.: porta 3001 ocupada pelo BarERP antigo)
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(5), {
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let server_error = ctx.server.as_ref().and_then(|srv| srv.startup_error());
            ui.set_server_online(ctx.server.is_some() && server_error.is_none());
            if let Some(error) = server_error {
                status(&ui, error, true);
            }
        }
    });
    timer
}
