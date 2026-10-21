//! Equipe de Garçons & Comissões (ManageWatersModal), Funcionários & Permissões (ManageStaffModal) e
//! Conectar App Android / Celular (ConnectMobileModal).

use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, SharedString};

use barerp_db::staff::{self, StaffInput, StaffMember};
use barerp_db::waiters::{self, Waiter, WaiterUpdate};

use super::{modal, modal_error};
use crate::ctx::AppCtx;
use crate::view::model;
use crate::{MesasModal, MesasModalStore, PermItem, StaffRow, WaiterRow};

/// Módulos de acesso, como no original (id, nome, descrição).
const MODULES: [(&str, &str, &str); 9] = [
    ("tables", "Mesas & Salão", "Abertura, consumo e fechamento de mesas"),
    ("kds", "KDS (Cozinha/Bar)", "Painel de produção de pedidos e comandas"),
    ("cash", "Caixa & PDV", "Operação de caixa, vendas rápidas e fechamento"),
    ("products", "Cardápio & Estoque", "Cadastro de produtos, preços e receitas"),
    ("fiscal", "Módulo Fiscal", "Emissão e cancelamento de NFC-e"),
    ("settings", "Retaguarda", "Painel de administração e configurações"),
    ("dashboard", "Métricas & Gestão", "Faturamento, relatórios e auditoria"),
    ("customers", "Clientes (Fiado)", "Cadastro de clientes e saldos devedores"),
    ("suppliers", "Fornecedores", "Cadastro de fornecedores e compras"),
];

const ROLES: [(&str, &str); 6] = [
    ("ADMIN", "Administrador (Acesso Total)"),
    ("GERENTE", "Gerente de Turno"),
    ("CAIXA", "Operador de Caixa / PDV"),
    ("GARCOM", "Atendente / Garçom"),
    ("COZINHA", "Cozinha / Bar KDS"),
    ("ESTOQUISTA", "Estoquista / Compras"),
];

thread_local! {
    static WAITERS: std::cell::RefCell<Vec<Waiter>> = const { std::cell::RefCell::new(Vec::new()) };
    static STAFF: std::cell::RefCell<(Vec<StaffMember>, Vec<String>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
}

fn role_label(code: &str) -> String {
    ROLES.iter().find(|(c, _)| *c == code).map(|(_, l)| l.to_string()).unwrap_or_else(|| code.to_string())
}

fn role_code(label: &str) -> String {
    ROLES.iter().find(|(_, l)| *l == label).map(|(c, _)| c.to_string()).unwrap_or_else(|| "OPERADOR".to_string())
}

// ---------- Garçons ----------

fn render_waiters(ui: &crate::AppWindow) {
    let rows: Vec<WaiterRow> = WAITERS.with(|w| {
        w.borrow()
            .iter()
            .map(|w| WaiterRow {
                id: w.id.clone().into(),
                name: w.name.clone().into(),
                badge: w
                    .code
                    .clone()
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| w.name.chars().take(2).collect::<String>().to_uppercase())
                    .into(),
                code: w.code.clone().unwrap_or_default().into(),
                commission: format!("{}%", (w.commission_rate * 100.0).round()).into(),
                active: w.active,
            })
            .collect()
    });
    ui.global::<MesasModalStore>().set_w_list(model(rows));
}

fn load_waiters(ctx: &AppCtx) {
    if let Some(ui) = ctx.ui() {
        ui.global::<MesasModalStore>().set_w_loading(true);
    }
    let db = ctx.db.clone();
    ctx.spawn(async move { waiters::list(&db, true).await }, |ui, result| {
        ui.global::<MesasModalStore>().set_w_loading(false);
        match result {
            Ok(list) => {
                WAITERS.with(|w| *w.borrow_mut() = list);
                render_waiters(ui);
            }
            Err(_) => modal_error(ui, "Não foi possível carregar a lista de garçons."),
        }
    });
}

fn clear_waiter_form(m: &MesasModalStore) {
    m.set_w_name(SharedString::new());
    m.set_w_code(SharedString::new());
    m.set_w_commission("10".into());
    m.set_w_editing_id(SharedString::new());
}

pub(crate) fn open_waiters(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    clear_waiter_form(&m);
    m.set_w_deleting_id(SharedString::new());
    modal(&ui, MesasModal::Waiters);
    load_waiters(ctx);
}

// ---------- Funcionários ----------

fn render_staff(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    STAFF.with(|s| {
        let (list, selected) = &*s.borrow();
        let rows: Vec<StaffRow> = list
            .iter()
            .map(|st| StaffRow {
                id: st.id.clone().into(),
                name: st.name.clone().into(),
                role: role_label(&st.role).into(),
                perms: st
                    .permissions
                    .iter()
                    .map(|p| MODULES.iter().find(|(id, _, _)| id == p).map(|(_, l, _)| *l).unwrap_or(p.as_str()))
                    .collect::<Vec<_>>()
                    .join(" · ")
                    .into(),
                active: st.active,
            })
            .collect();
        m.set_s_list(model(rows));
        let perms: Vec<PermItem> = MODULES
            .iter()
            .map(|(id, label, desc)| PermItem {
                id: (*id).into(),
                label: (*label).into(),
                desc: (*desc).into(),
                on: selected.iter().any(|p| p == id),
            })
            .collect();
        m.set_s_perms(model(perms));
    });
}

fn load_staff(ctx: &AppCtx) {
    let db = ctx.db.clone();
    ctx.spawn(async move { staff::list_all(&db).await }, |ui, result| match result {
        Ok(list) => {
            STAFF.with(|s| s.borrow_mut().0 = list);
            render_staff(ui);
        }
        Err(e) => modal_error(ui, e.to_string()),
    });
}

fn clear_staff_form(m: &MesasModalStore) {
    m.set_s_name(SharedString::new());
    m.set_s_password(SharedString::new());
    m.set_s_role(role_label("CAIXA").into());
    m.set_s_active(true);
    m.set_s_editing_id(SharedString::new());
    STAFF.with(|s| s.borrow_mut().1 = vec!["tables".into(), "cash".into()]);
}

pub(crate) fn open_staff(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    clear_staff_form(&m);
    m.set_s_deleting_id(SharedString::new());
    render_staff(&ui);
    modal(&ui, MesasModal::Staff);
    load_staff(ctx);
}

// ---------- Conectar celular ----------

fn qr_image(text: &str) -> Option<Image> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let quiet = 2;
    let scale = 6;
    let size = (width + quiet * 2) * scale;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(size as u32, size as u32);
    let pixels = buffer.make_mut_slice();
    for y in 0..size {
        for x in 0..size {
            let (mx, my) = (x / scale, y / scale);
            let dark = mx >= quiet
                && my >= quiet
                && mx < width + quiet
                && my < width + quiet
                && code[(mx - quiet, my - quiet)] == qrcode::Color::Dark;
            let v = if dark { 0 } else { 255 };
            pixels[y * size + x] = Rgba8Pixel { r: v, g: v, b: v, a: 255 };
        }
    }
    Some(Image::from_rgba8(buffer))
}

pub(crate) fn open_connect(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();
    let (ips, port) = match &ctx.server {
        Some(server) => (server.local_ips(), server.port()),
        None => (Vec::new(), 3001),
    };
    let primary = ips.first().cloned().unwrap_or_else(|| "192.168.0.x".into());
    let url = format!("http://{primary}:{port}");
    if let Some(image) = qr_image(&url) {
        m.set_x_qr(image);
    }
    m.set_x_url(url.into());
    m.set_x_status(primary.into());
    let others: Vec<SharedString> = ips.iter().skip(1).map(|ip| format!("Outra rede: http://{ip}:{port}").into()).collect();
    m.set_x_ips(model(others));
    modal(&ui, MesasModal::Connect);
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let m = ui.global::<MesasModalStore>();

    // Garçons
    m.on_w_save({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let m = ui.global::<MesasModalStore>();
            let name = m.get_w_name().trim().to_string();
            if name.is_empty() {
                modal_error(&ui, "Informe o nome do garçom");
                return;
            }
            let code = m.get_w_code().trim().to_string();
            let rate = m.get_w_commission().trim().replace(',', ".").parse::<f64>().unwrap_or(10.0) / 100.0;
            let editing = m.get_w_editing_id().to_string();
            m.set_busy(true);
            m.set_error(SharedString::new());
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(
                async move {
                    if editing.is_empty() {
                        waiters::create(&db, &name, Some(code.as_str()).filter(|c| !c.is_empty()), Some(rate)).await.map(|_| ())
                    } else {
                        waiters::update(
                            &db,
                            &editing,
                            WaiterUpdate { name: Some(name), code: Some(code), commission_rate: Some(rate), active: None },
                        )
                        .await
                        .map(|_| ())
                    }
                },
                move |ui, result| {
                    ui.global::<MesasModalStore>().set_busy(false);
                    match result {
                        Ok(()) => {
                            clear_waiter_form(&ui.global::<MesasModalStore>());
                            load_waiters(&next);
                        }
                        Err(e) => modal_error(ui, e.to_string()),
                    }
                },
            );
        }
    });
    m.on_w_edit({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let Some(w) = WAITERS.with(|w| w.borrow().iter().find(|w| w.id == id.as_str()).cloned()) else { return };
            let m = ui.global::<MesasModalStore>();
            m.set_w_editing_id(w.id.into());
            m.set_w_name(w.name.into());
            m.set_w_code(w.code.unwrap_or_default().into());
            m.set_w_commission(format!("{}", (w.commission_rate * 100.0).round()).into());
        }
    });
    m.on_w_cancel({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                clear_waiter_form(&ui.global::<MesasModalStore>());
            }
        }
    });
    m.on_w_toggle({
        let ctx = ctx.clone();
        move |id| {
            let Some(active) = WAITERS.with(|w| w.borrow().iter().find(|w| w.id == id.as_str()).map(|w| w.active)) else { return };
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            ctx.spawn(
                async move { waiters::update(&db, &id, WaiterUpdate { active: Some(!active), ..Default::default() }).await },
                move |ui, result| match result {
                    Ok(_) => load_waiters(&next),
                    Err(e) => modal_error(ui, e.to_string()),
                },
            );
        }
    });
    m.on_w_delete({
        let ctx = ctx.clone();
        move |id| {
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            ctx.spawn(async move { waiters::remove(&db, &id).await }, move |ui, result| {
                ui.global::<MesasModalStore>().set_w_deleting_id(SharedString::new());
                match result {
                    Ok(r) => {
                        next.status(r.message, false);
                        load_waiters(&next);
                    }
                    Err(e) => modal_error(ui, e.to_string()),
                }
            });
        }
    });

    // Funcionários
    m.on_s_preset({
        let ctx = ctx.clone();
        move |preset| {
            let perms: Vec<String> = match preset.as_str() {
                "ALL" => MODULES.iter().map(|(id, _, _)| id.to_string()).collect(),
                "CAIXA" => vec!["tables".into(), "cash".into(), "customers".into()],
                "GARCOM" => vec!["tables".into()],
                _ => vec!["kds".into()],
            };
            STAFF.with(|s| s.borrow_mut().1 = perms);
            if let Some(ui) = ctx.ui() {
                render_staff(&ui);
            }
        }
    });
    m.on_s_toggle_perm({
        let ctx = ctx.clone();
        move |index| {
            let Some((id, _, _)) = MODULES.get(index as usize) else { return };
            STAFF.with(|s| {
                let selected = &mut s.borrow_mut().1;
                if let Some(pos) = selected.iter().position(|p| p == id) {
                    selected.remove(pos);
                } else {
                    selected.push(id.to_string());
                }
            });
            if let Some(ui) = ctx.ui() {
                render_staff(&ui);
            }
        }
    });
    m.on_s_edit({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let Some(st) = STAFF.with(|s| s.borrow().0.iter().find(|s| s.id == id.as_str()).cloned()) else { return };
            let m = ui.global::<MesasModalStore>();
            m.set_s_editing_id(st.id.into());
            m.set_s_name(st.name.into());
            m.set_s_role(role_label(&st.role).into());
            m.set_s_password(SharedString::new());
            m.set_s_active(st.active);
            STAFF.with(|s| s.borrow_mut().1 = st.permissions);
            render_staff(&ui);
        }
    });
    m.on_s_cancel({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                clear_staff_form(&ui.global::<MesasModalStore>());
                render_staff(&ui);
            }
        }
    });
    m.on_s_save({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let m = ui.global::<MesasModalStore>();
            let editing = m.get_s_editing_id().to_string();
            let input = StaffInput {
                name: m.get_s_name().to_string(),
                role: role_code(&m.get_s_role()),
                password: m.get_s_password().to_string(),
                permissions: STAFF.with(|s| s.borrow().1.clone()),
                active: m.get_s_active(),
            };
            if input.name.trim().is_empty() {
                modal_error(&ui, "Informe o nome do funcionário");
                return;
            }
            if editing.is_empty() && input.password.trim().is_empty() {
                modal_error(&ui, "Defina uma senha ou PIN de acesso para o novo funcionário");
                return;
            }
            m.set_busy(true);
            m.set_error(SharedString::new());
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(
                async move {
                    if editing.is_empty() {
                        staff::create_staff(&db, input).await.map(|_| ())
                    } else {
                        staff::update_staff(&db, &editing, input).await
                    }
                },
                move |ui, result| {
                    ui.global::<MesasModalStore>().set_busy(false);
                    match result {
                        Ok(()) => {
                            clear_staff_form(&ui.global::<MesasModalStore>());
                            next.status("Funcionário salvo.", false);
                            load_staff(&next);
                        }
                        Err(e) => modal_error(ui, e.to_string()),
                    }
                },
            );
        }
    });
    m.on_s_delete({
        let ctx = ctx.clone();
        move |id| {
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            ctx.spawn(async move { staff::delete_staff(&db, &id).await }, move |ui, result| {
                ui.global::<MesasModalStore>().set_s_deleting_id(SharedString::new());
                match result {
                    Ok(()) => {
                        next.status("Funcionário removido com sucesso", false);
                        load_staff(&next);
                    }
                    Err(e) => modal_error(ui, e.to_string()),
                }
            });
        }
    });
}

/// Gerador de prints: abre as janelas de equipe com listas já carregadas.
pub(crate) fn preview_waiters(ui: &crate::AppWindow, list: Vec<Waiter>) {
    WAITERS.with(|w| *w.borrow_mut() = list);
    clear_waiter_form(&ui.global::<MesasModalStore>());
    render_waiters(ui);
    modal(ui, MesasModal::Waiters);
}

pub(crate) fn preview_staff(ui: &crate::AppWindow, list: Vec<StaffMember>) {
    STAFF.with(|s| s.borrow_mut().0 = list);
    clear_staff_form(&ui.global::<MesasModalStore>());
    render_staff(ui);
    modal(ui, MesasModal::Staff);
}

pub(crate) fn preview_connect(ui: &crate::AppWindow) {
    let m = ui.global::<MesasModalStore>();
    let url = "http://192.168.0.15:3001";
    if let Some(image) = qr_image(url) {
        m.set_x_qr(image);
    }
    m.set_x_url(url.into());
    m.set_x_status("192.168.0.15".into());
    m.set_x_ips(model(Vec::new()));
    modal(ui, MesasModal::Connect);
}
