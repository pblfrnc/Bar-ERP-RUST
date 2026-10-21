//! Atualização do app (UpdateNotificationModal.tsx + `electron/updater.cjs`): confere 15 s depois de abrir e a
//! cada 4 horas, mostra o aviso e a janela, baixa o instalador com progresso e o executa fechando o app.
//! A checagem manual fica no cartão "Bar ERP Pro Desktop" da Retaguarda ([`check_now`]).

use std::cell::RefCell;
use std::time::Duration;

use chrono::Local;
use slint::{ComponentHandle, SharedString, Timer, TimerMode};

use barerp_updater::{Endpoints, LocalBuild, UpdateInfo};

use crate::ctx::AppCtx;
use crate::{AppWindow, RetaguardaStore, UpdateStore};

thread_local! {
    static INFO: RefCell<Option<UpdateInfo>> = const { RefCell::new(None) };
    static TIMERS: RefCell<Vec<Timer>> = const { RefCell::new(Vec::new()) };
}

fn apply(ui: &AppWindow, info: &UpdateInfo) {
    let s = ui.global::<UpdateStore>();
    s.set_has_update(info.has_update);
    s.set_latest_version(info.latest_version.clone().into());
    s.set_current_version(info.current_version.clone().into());
    s.set_notes(
        if info.release_notes.is_empty() {
            "Melhorias de desempenho, correções e novas funcionalidades integradas.".to_string()
        } else {
            info.release_notes.clone()
        }
        .into(),
    );
    s.set_release_date(
        info.asset_date
            .map(|d| d.with_timezone(&Local).format("%d/%m/%Y, %H:%M").to_string())
            .unwrap_or_else(|| "Recentemente".into())
            .into(),
    );
    let r = ui.global::<RetaguardaStore>();
    r.set_has_update(info.has_update);
    if info.has_update {
        r.set_update_text(format!("Uma nova versão ({}) está pronta para instalar!", info.latest_version).into());
    }
    INFO.with(|i| *i.borrow_mut() = Some(info.clone()));
}

fn check(ctx: &AppCtx, manual: bool) {
    if manual {
        if let Some(ui) = ctx.ui() {
            ui.global::<RetaguardaStore>().set_checking_update(true);
        }
    }
    ctx.spawn(
        async move { barerp_updater::check(&Endpoints::default(), &LocalBuild::current()).await },
        move |ui, info| {
            let r = ui.global::<RetaguardaStore>();
            r.set_checking_update(false);
            // Aviso automático só quando há novidade; a checagem manual sempre responde
            if info.has_update || manual {
                apply(ui, &info);
            }
            if manual {
                if info.has_update {
                    r.set_update_text(format!("Nova versão {} disponível!", info.latest_version).into());
                    ui.global::<UpdateStore>().set_modal_open(true);
                } else {
                    // O resultado fica na tela com a hora da checagem (antes sumia em 6 s e parecia que nada acontecia)
                    let text = info.message.clone().unwrap_or_else(|| "Você já está usando a versão mais recente do Bar ERP.".into());
                    r.set_update_text(format!("{text} Verificado às {}.", Local::now().format("%H:%M")).into());
                }
            }
        },
    );
}

/// "Verificar Atualizações" da Retaguarda.
pub fn check_now(ctx: &AppCtx) {
    check(ctx, true);
}

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = ui.global::<UpdateStore>();
    s.set_current_version(env!("CARGO_PKG_VERSION").into());

    let c = ctx.clone();
    s.on_start_download(move || {
        let Some(ui) = c.ui() else { return };
        let Some(info) = INFO.with(|i| i.borrow().clone()) else { return };
        let s = ui.global::<UpdateStore>();
        s.set_downloading(true);
        s.set_complete(false);
        s.set_error(SharedString::new());
        s.set_percent(0);
        s.set_progress_text(format!("0.0 MB de {:.1} MB", info.file_size as f64 / 1_048_576.0).into());
        s.set_speed_text("Velocidade: 0.0 MB/s".into());
        let weak = ui.as_weak();
        c.spawn(
            async move {
                let target = barerp_updater::installer_path();
                barerp_updater::download(&info.download_url, &target, move |p| {
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        let s = ui.global::<UpdateStore>();
                        s.set_percent(p.percent as i32);
                        s.set_progress_text(
                            format!("{:.1} MB de {:.1} MB", p.received as f64 / 1_048_576.0, p.total as f64 / 1_048_576.0).into(),
                        );
                        s.set_speed_text(format!("Velocidade: {:.1} MB/s", p.speed_mbps).into());
                    });
                })
                .await
            },
            |ui, result| {
                let s = ui.global::<UpdateStore>();
                s.set_downloading(false);
                match result {
                    Ok(_) => s.set_complete(true),
                    Err(e) => s.set_error(e.to_string().into()),
                }
            },
        );
    });

    s.on_install(move || {
        match barerp_updater::install(&barerp_updater::installer_path()) {
            // Fecha o app logo em seguida para liberar os arquivos travados (EBUSY no Electron)
            Ok(()) => {
                Timer::single_shot(Duration::from_millis(500), || {
                    let _ = slint::quit_event_loop();
                });
            }
            Err(e) => {
                if let Some(ui) = ui_weak_upgrade() {
                    ui.global::<UpdateStore>().set_error(e.to_string().into());
                }
            }
        }
    });
    UI.with(|u| *u.borrow_mut() = Some(ui.as_weak()));

    // 15 s depois de abrir e a cada 4 horas
    let c = ctx.clone();
    Timer::single_shot(Duration::from_secs(15), move || check(&c, false));
    let timer = Timer::default();
    let c = ctx.clone();
    timer.start(TimerMode::Repeated, Duration::from_secs(4 * 60 * 60), move || check(&c, false));
    TIMERS.with(|t| t.borrow_mut().push(timer));
}

thread_local! {
    static UI: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

fn ui_weak_upgrade() -> Option<AppWindow> {
    UI.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade()))
}

/// Para os prints das telas, sem rede.
pub mod preview {
    use super::*;

    pub fn show(ui: &AppWindow, info: &UpdateInfo, open: bool) {
        apply(ui, info);
        ui.global::<UpdateStore>().set_modal_open(open);
    }
}
