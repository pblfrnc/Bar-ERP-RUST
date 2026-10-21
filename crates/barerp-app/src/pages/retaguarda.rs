//! Página "retaguarda": SettingsView (hub), DashboardView + AuditView, PrinterSettingsView, SubscriptionView,
//! LicenseModal (bloqueio total), BackupSettingsCard e DigitalMenuCard. Ver `paridade/retaguarda.md`.
//!
//! Também roda os serviços de fundo do original: backup automático (ao abrir, a cada 6 h e ao fechar o caixa),
//! publicação do cardápio digital (depois de mudanças no cadastro e a cada 5 min), checagem da licença
//! (a cada 60 s, com o painel do Render a cada 2 min) e a auto-impressão da comanda da cozinha.

use std::cell::RefCell;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode};

use barerp_backup::{BackupService, BackupSettingsInput, BackupStatus};
use barerp_core::money::brl;
use barerp_db::dashboard::{self, DashboardData};
use barerp_db::printer::{self, PrinterSettings};
use barerp_license::LicenseStatus;
use barerp_menu_publish::db_store::DbMenuStore;
use barerp_menu_publish::{DigitalMenuStatus, MenuPublisher};

use crate::ctx::{open_external, AppCtx, Preferences};
use crate::view::model;
use crate::{
    AppWindow, CardapioStore, MesasStore, Page, RetaguardaStore, RtAudit, RtBar, RtDevice, RtLowMargin, RtMenuCategory,
    RtMenuRow, RtTopProduct, RtView, RtWaiter,
};

// ======================================================================
// Estado
// ======================================================================

struct State {
    machine_id: Option<String>,
    license: Option<LicenseStatus>,
    checking_remote: bool,
    margin_loaded: bool,
    day: NaiveDate,
    categories: Vec<(String, String)>,
    hidden: Vec<String>,
    menu: Option<DigitalMenuStatus>,
    printer: PrinterSettings,
}

impl Default for State {
    fn default() -> Self {
        Self {
            machine_id: None,
            license: None,
            checking_remote: false,
            margin_loaded: false,
            day: Local::now().date_naive(),
            categories: Vec::new(),
            hidden: Vec::new(),
            menu: None,
            printer: PrinterSettings::default(),
        }
    }
}

#[derive(Clone)]
struct Services {
    backup: Option<BackupService>,
    menu: MenuPublisher<DbMenuStore>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static SERVICES: RefCell<Option<Services>> = const { RefCell::new(None) };
    static TIMERS: RefCell<Vec<Timer>> = const { RefCell::new(Vec::new()) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn services() -> Option<Services> {
    SERVICES.with(|s| s.borrow().clone())
}

fn store(ui: &AppWindow) -> RetaguardaStore<'_> {
    ui.global::<RetaguardaStore>()
}

fn alert(ui: &AppWindow, text: impl Into<SharedString>) {
    store(ui).set_alert_text(text.into());
}

/// Mantém um timer de uso único vivo até disparar (copiado!, salvo com sucesso!).
fn later(delay: Duration, f: impl FnOnce() + 'static) {
    Timer::single_shot(delay, f);
}

// ======================================================================
// Formatação (pt-BR do original)
// ======================================================================

const MONTHS: [&str; 12] =
    ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];
const WEEKDAYS: [&str; 7] = ["seg.", "ter.", "qua.", "qui.", "sex.", "sáb.", "dom."];

/// `toLocaleDateString('pt-BR', { day: '2-digit', month: 'long', year: 'numeric' })`: "05 de janeiro de 2027".
fn long_date(at: DateTime<Utc>) -> String {
    let d = at.with_timezone(&Local);
    format!("{:02} de {} de {}", d.day(), MONTHS[d.month0() as usize], d.year())
}

fn short_date(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%d/%m/%Y").to_string()
}

/// `toLocaleString('pt-BR', { dateStyle: 'short', timeStyle: 'short' })` de um ISO; "nunca" sem data.
fn date_time_iso(iso: Option<&str>) -> String {
    iso.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Local).format("%d/%m/%Y, %H:%M").to_string())
        .unwrap_or_else(|| "nunca".into())
}

fn method_label(method: &str) -> String {
    match method {
        "PIX" => "PIX".into(),
        "CASH" => "Dinheiro".into(),
        "CREDIT_CARD" => "Cartão Crédito".into(),
        "DEBIT_CARD" => "Cartão Débito".into(),
        "VOUCHER" => "Voucher / Vale".into(),
        "CREDIT_TAB" => "Conta Fiado".into(),
        other => other.into(),
    }
}

fn days_text(days: i64) -> String {
    format!("{days} {}", if days == 1 { "dia restante" } else { "dias restantes" })
}

/// Valor digitado com vírgula ou ponto.
fn parse_number(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

fn qr_image(text: &str) -> Option<Image> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let scale = 6;
    let size = width * scale;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(size as u32, size as u32);
    let pixels = buffer.make_mut_slice();
    for y in 0..size {
        for x in 0..size {
            let dark = code[(x / scale, y / scale)] == qrcode::Color::Dark;
            let v = if dark { 0 } else { 255 };
            pixels[y * size + x] = Rgba8Pixel { r: v, g: v, b: v, a: 255 };
        }
    }
    Some(Image::from_rgba8(buffer))
}

fn whatsapp(phone: &str, text: &str) {
    let digits: String = phone.chars().filter(char::is_ascii_digit).collect();
    let phone = if digits.starts_with("55") { digits } else { format!("55{digits}") };
    open_external(&format!("https://wa.me/{phone}?text={}", url_encode(text)));
}

/// `encodeURIComponent`
fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ======================================================================
// Licença
// ======================================================================

/// Lê o status da licença e atualiza a tela de ativação, a barra superior, o hub e a assinatura.
/// Se a última verificação no painel tem mais de 2 minutos, revalida em segundo plano (como o GET do Node).
fn refresh_license(ctx: &AppCtx) {
    let Some(mid) = with_state(|s| s.machine_id.clone()) else { return };
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(async move { barerp_license::status(&db, &mid).await }, move |ui, result| match result {
        Ok(st) => {
            apply_license(ui, &st);
            let start_check = st.should_check_remote && !with_state(|s| std::mem::replace(&mut s.checking_remote, true));
            if start_check {
                let (db, mid, c2) = (c.db.clone(), st.machine_id.clone(), c.clone());
                c.spawn(async move { barerp_license::background_check(&db, &mid).await }, move |_, r| {
                    with_state(|s| s.checking_remote = false);
                    if let Err(e) = r {
                        tracing::warn!("[Licensing] {e}");
                    }
                    refresh_license_quiet(&c2);
                });
            }
        }
        Err(e) => tracing::warn!("erro ao verificar licença: {e}"),
    });
}

/// Relê sem disparar outra checagem no painel.
fn refresh_license_quiet(ctx: &AppCtx) {
    let Some(mid) = with_state(|s| s.machine_id.clone()) else { return };
    let db = ctx.db.clone();
    ctx.spawn(async move { barerp_license::status(&db, &mid).await }, |ui, result| {
        if let Ok(st) = result {
            apply_license(ui, &st);
        }
    });
}

fn apply_license(ui: &AppWindow, st: &LicenseStatus) {
    let s = store(ui);
    let days = st.days_remaining;
    s.set_g_visible(st.blocked());
    s.set_g_status(st.license_status.clone().into());
    ui.set_license_days(if st.is_expiring_soon { days as i32 } else { -1 });

    s.set_license_badge(days_text(days).into());
    s.set_license_badge_warn(days <= 5);
    let short: String = st.machine_id.chars().take(16).collect();
    s.set_machine_short(format!("{short}...").into());

    s.set_s_days(days as i32);
    s.set_s_status_label(if st.license_status == "ACTIVE" { "Ativa & Em Operação".into() } else { st.license_status.clone().into() });
    s.set_s_soon(days <= 5 && days > 0);
    s.set_s_expiration(st.expires_at.map(long_date).unwrap_or_else(|| "Data em validação".into()).into());
    s.set_s_client(st.client_name.clone().unwrap_or_default().into());
    s.set_s_machine(st.machine_id.clone().into());
    s.set_s_pix(st.developer_contact.pix_key.clone().into());
    s.set_s_dev_name(st.developer_contact.developer_name.clone().into());
    s.set_i_days(days as i32);
    if let Some(url) = st.license_server_url.as_deref().filter(|u| !u.is_empty()) {
        if s.get_g_server_url() == barerp_license::DEFAULT_SERVER_URL {
            s.set_g_server_url(url.into());
        }
    }
    if !with_state(|x| std::mem::replace(&mut x.margin_loaded, true)) {
        s.set_margin_input(format!("{}", st.default_profit_margin).into());
    }
    with_state(|x| x.license = Some(st.clone()));
    render_projection(ui);
}

fn render_projection(ui: &AppWindow) {
    let s = store(ui);
    let months = s.get_s_months().max(1) as i64;
    let (expires, days) = with_state(|x| x.license.as_ref().map(|l| (l.expires_at, l.days_remaining)).unwrap_or((None, 0)));
    let base = expires.unwrap_or_else(Utc::now);
    let future = base + chrono::Duration::days(months * 30);
    s.set_s_projection(format!("Válido até {} ({} dias no total)", long_date(future), days + months * 30).into());
}

fn license_phone() -> String {
    with_state(|s| s.license.as_ref().map(|l| l.developer_contact.phone.clone())).unwrap_or_else(|| "5547974002560".into())
}

fn machine_id() -> String {
    with_state(|s| s.machine_id.clone()).unwrap_or_default()
}

fn bind_license(ctx: &AppCtx, s: &RetaguardaStore) {
    // Tela de ativação
    let c = ctx.clone();
    s.on_g_copy_id(move || {
        crate::ctx::copy_text(&machine_id());
        if let Some(ui) = c.ui() {
            store(&ui).set_g_copied_id(true);
            let w = ui.as_weak();
            later(Duration::from_secs(2), move || {
                if let Some(ui) = w.upgrade() {
                    store(&ui).set_g_copied_id(false);
                }
            });
        }
    });
    let c = ctx.clone();
    s.on_g_copy_pix(move || {
        let Some(ui) = c.ui() else { return };
        crate::ctx::copy_text(&store(&ui).get_s_pix());
        store(&ui).set_g_copied_pix(true);
        let w = ui.as_weak();
        later(Duration::from_secs(2), move || {
            if let Some(ui) = w.upgrade() {
                store(&ui).set_g_copied_pix(false);
            }
        });
    });
    s.on_g_whatsapp(move || {
        let text = format!(
            "Olá! Segue o comprovante de pagamento da mensalidade do Bar ERP.\n\n💻 *ID DA MINHA MÁQUINA:*\n`{}`\n\nPor favor, ative minha licença remotamente. Obrigado!",
            machine_id()
        );
        whatsapp(&license_phone(), &text);
    });
    let c = ctx.clone();
    s.on_g_check_online(move || {
        let Some(ui) = c.ui() else { return };
        let s = store(&ui);
        s.set_g_syncing(true);
        s.set_g_error(SharedString::new());
        s.set_g_success(SharedString::new());
        let url = s.get_g_server_url().trim().to_string();
        let (db, mid, c2) = (c.db.clone(), machine_id(), c.clone());
        c.spawn(
            async move { barerp_license::sync_remote(&db, &mid, Some(url.as_str()).filter(|u| !u.is_empty())).await },
            move |ui, result| {
                let s = store(ui);
                s.set_g_syncing(false);
                match result {
                    Ok(out) if out.activated => {
                        s.set_g_success("🎉 Licença ativada remotamente com sucesso! Liberando sistema...".into());
                        later(Duration::from_millis(1500), move || refresh_license_quiet(&c2));
                    }
                    // Painel fora do ar com licença local válida: o original também mostrava como aviso
                    Ok(out) => s.set_g_error(out.message.into()),
                    Err(e) => s.set_g_error(e.to_string().into()),
                }
            },
        );
    });
    let c = ctx.clone();
    s.on_g_activate_offline(move || {
        let Some(ui) = c.ui() else { return };
        let s = store(&ui);
        let key = s.get_g_offline_key().trim().to_string();
        if key.is_empty() {
            s.set_g_error("Informe a chave de ativação offline recebida no WhatsApp.".into());
            return;
        }
        s.set_g_offline_busy(true);
        s.set_g_error(SharedString::new());
        s.set_g_success(SharedString::new());
        let (db, mid, c2) = (c.db.clone(), machine_id(), c.clone());
        c.spawn(async move { barerp_license::activate_offline(&db, &mid, &key).await }, move |ui, result| {
            let s = store(ui);
            s.set_g_offline_busy(false);
            match result {
                Ok(_) => {
                    s.set_g_success("🎉 Licença offline ativada com sucesso! Liberando sistema...".into());
                    later(Duration::from_millis(1500), move || refresh_license_quiet(&c2));
                }
                Err(e) => s.set_g_error(e.to_string().into()),
            }
        });
    });

    // Aviso de mensalidade (barra superior)
    s.on_i_whatsapp(move || {
        let text = format!("Olá! Segue o comprovante de renovação da mensalidade do Bar ERP.\nID da minha máquina: {}", machine_id());
        whatsapp(&license_phone(), &text);
    });
    let c = ctx.clone();
    s.on_i_open_subscription(move || {
        if let Some(ui) = c.ui() {
            ui.set_page(Page::Settings);
            store(&ui).set_view(RtView::Subscription);
        }
    });

    // Assinatura
    let c = ctx.clone();
    s.on_s_sync(move || {
        let Some(ui) = c.ui() else { return };
        store(&ui).set_s_syncing(true);
        store(&ui).set_s_sync_message(SharedString::new());
        let (db, mid, c2) = (c.db.clone(), machine_id(), c.clone());
        c.spawn(
            async move {
                let out = barerp_license::sync_remote(&db, &mid, None).await?;
                let st = barerp_license::status(&db, &mid).await?;
                Ok::<_, barerp_license::LicenseError>((out, st))
            },
            move |ui, result| {
                let s = store(ui);
                s.set_s_syncing(false);
                match result {
                    Ok((out, st)) => {
                        let validity = st.expires_at.map(short_date).unwrap_or_else(|| "Ativa".into());
                        let days = out.days_remaining.unwrap_or(st.days_remaining);
                        s.set_s_sync_ok(true);
                        s.set_s_sync_message(format!("🎉 Licença sincronizada! Nova validade: {validity} ({days} dias restantes).").into());
                        apply_license(ui, &st);
                    }
                    Err(e) => {
                        s.set_s_sync_ok(false);
                        s.set_s_sync_message(e.to_string().into());
                        refresh_license_quiet(&c2);
                    }
                }
                let w = ui.as_weak();
                later(Duration::from_secs(8), move || {
                    if let Some(ui) = w.upgrade() {
                        store(&ui).set_s_sync_message(SharedString::new());
                    }
                });
            },
        );
    });
    let c = ctx.clone();
    s.on_s_copy_id(move || {
        let mid = machine_id();
        if mid.is_empty() {
            return;
        }
        crate::ctx::copy_text(&mid);
        if let Some(ui) = c.ui() {
            store(&ui).set_s_copied_id(true);
            let w = ui.as_weak();
            later(Duration::from_secs(2), move || {
                if let Some(ui) = w.upgrade() {
                    store(&ui).set_s_copied_id(false);
                }
            });
        }
    });
    let c = ctx.clone();
    s.on_s_copy_pix(move || {
        let Some(ui) = c.ui() else { return };
        crate::ctx::copy_text(&store(&ui).get_s_pix());
        store(&ui).set_s_copied_pix(true);
        let w = ui.as_weak();
        later(Duration::from_secs(2), move || {
            if let Some(ui) = w.upgrade() {
                store(&ui).set_s_copied_pix(false);
            }
        });
    });
    let c = ctx.clone();
    s.on_s_set_months(move |m| {
        if let Some(ui) = c.ui() {
            store(&ui).set_s_months(m);
            render_projection(&ui);
        }
    });
    let c = ctx.clone();
    s.on_s_request_renewal(move || {
        let Some(ui) = c.ui() else { return };
        let s = store(&ui);
        let months = s.get_s_months();
        let (license, days) = with_state(|x| (x.license.clone(), x.license.as_ref().map(|l| l.days_remaining).unwrap_or(0)));
        let dev_name = s.get_s_dev_name().to_string();
        let client = license.as_ref().and_then(|l| l.client_name.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "Cliente Bar ERP".into());
        let current = license.as_ref().and_then(|l| l.expires_at).map(short_date).unwrap_or_else(|| format!("{days} dias"));
        let period = match months {
            1 => "1 Mês (+30 dias)".to_string(),
            2 => "2 Meses (+60 dias)".to_string(),
            3 => "3 Meses / Trimestral (+90 dias)".to_string(),
            6 => "6 Meses / Semestral (+180 dias)".to_string(),
            12 => "12 Meses / Anual (+365 dias)".to_string(),
            n => format!("{n} Meses"),
        };
        let message = [
            format!("👋 *Olá, {dev_name}!*"),
            "Gostaria de solicitar a *renovação antecipada* da licença do *Bar ERP*.".into(),
            String::new(),
            format!("🏢 *Estabelecimento:* {client}"),
            format!("💻 *ID da Máquina:* `{}`", machine_id()),
            format!("⏳ *Validade Atual:* {days} dias restantes (vence em: {current})"),
            format!("📅 *Período Antecipado:* {period}"),
            String::new(),
            "💰 *Forma:* Pagamento via PIX".into(),
            "Segue o comprovante de pagamento para liberação dos dias adicionais no sistema! Obrigado!".into(),
        ]
        .join("\n");
        whatsapp(&license_phone(), &message);
    });
    let c = ctx.clone();
    s.on_s_activate_offline(move || {
        let Some(ui) = c.ui() else { return };
        let s = store(&ui);
        let key = s.get_s_offline_key().trim().to_string();
        if key.is_empty() {
            return;
        }
        s.set_s_offline_busy(true);
        s.set_s_offline_message(SharedString::new());
        let (db, mid) = (c.db.clone(), machine_id());
        let c2 = c.clone();
        c.spawn(
            async move {
                barerp_license::activate_offline(&db, &mid, &key).await?;
                barerp_license::status(&db, &mid).await
            },
            move |ui, result| {
                let s = store(ui);
                s.set_s_offline_busy(false);
                match result {
                    Ok(st) => {
                        let until = st.expires_at.map(short_date).unwrap_or_else(|| "Ativo".into());
                        s.set_s_offline_ok(true);
                        s.set_s_offline_message(format!("🎉 Chave offline validada com sucesso! Válido até: {until}.").into());
                        s.set_s_offline_key(SharedString::new());
                        apply_license(ui, &st);
                    }
                    Err(e) => {
                        s.set_s_offline_ok(false);
                        s.set_s_offline_message(e.to_string().into());
                        refresh_license_quiet(&c2);
                    }
                }
            },
        );
    });
    s.on_s_support(move || whatsapp(&license_phone(), "Olá! Preciso de suporte no Bar ERP."));
}

// ======================================================================
// Hub: margem, backup, cardápio digital, dispositivos
// ======================================================================

fn load_hub(ctx: &AppCtx) {
    refresh_license_quiet(ctx);
    load_backup(ctx, true);
    load_menu(ctx, true);
    refresh_devices(ctx);
}

fn refresh_devices(ctx: &AppCtx) {
    let (Some(ui), Some(server)) = (ctx.ui(), ctx.server.as_ref()) else { return };
    let list = server
        .devices()
        .into_iter()
        .map(|d| RtDevice {
            label: match d.client_type.as_str() {
                "GARCOM_MOBILE" => "📱 App Garçom".into(),
                "COZINHA_KDS" => "📺 Tela Cozinha (KDS)".into(),
                other => format!("💻 {other}").into(),
            },
            kind: d.client_type.into(),
            waiter: d.waiter_name.unwrap_or_default().into(),
            agent: d.user_agent.into(),
            ip: d.ip.into(),
        })
        .collect();
    store(&ui).set_devices(model(list));
}

fn save_margin(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store(&ui);
    let Some(value) = parse_number(&s.get_margin_input()).filter(|v| *v > 0.0 && *v < 100.0) else {
        alert(&ui, "Informe uma margem de lucro válida entre 1% e 99%.");
        return;
    };
    s.set_margin_saving(true);
    let db = ctx.db.clone();
    ctx.spawn(async move { barerp_db::retaguarda::set_default_profit_margin(&db, value).await }, |ui, result| {
        store(ui).set_margin_saving(false);
        match result {
            Ok(()) => alert(ui, "Margem de lucro padrão do sistema salva com sucesso!"),
            Err(e) => alert(ui, format!("Erro ao salvar margem de lucro padrão: {e}")),
        }
    });
}

// ---------- Backup ----------

fn render_backup(ui: &AppWindow, st: &BackupStatus, sync_form: bool) {
    let s = store(ui);
    let mut alert_text: Option<String> = None;
    if st.enabled {
        let stale = st
            .last_external
            .as_ref()
            .and_then(|e| DateTime::parse_from_rfc3339(&e.at).ok())
            .is_some_and(|at| Utc::now().signed_duration_since(at.with_timezone(&Utc)) > chrono::Duration::hours(24));
        if let Some(err) = &st.last_error {
            alert_text = Some(err.clone());
        } else if !st.external_reachable {
            alert_text = Some(
                "A pasta de backup externo não está acessível. Verifique o pen drive ou a pasta do Google Drive/OneDrive.".into(),
            );
        } else if st.last_external.is_none() {
            alert_text = Some("Ainda não há backup na pasta externa. Clique em \"Fazer backup agora\".".into());
        } else if stale {
            alert_text = Some(format!(
                "O último backup externo tem mais de 24 horas ({}).",
                date_time_iso(st.last_external.as_ref().map(|e| e.at.as_str()))
            ));
        }
    } else if let Some(err) = &st.last_error {
        alert_text = Some(err.clone());
    }
    s.set_b_alert_on(alert_text.is_some());
    s.set_b_alert(alert_text.unwrap_or_default().into());
    s.set_b_last_local(date_time_iso(st.last_local.as_ref().map(|e| e.at.as_str())).into());
    s.set_b_local_count(format!("{} cópias guardadas", st.total_local).into());
    if st.enabled {
        s.set_b_last_external(date_time_iso(st.last_external.as_ref().map(|e| e.at.as_str())).into());
        s.set_b_external_sub(format!("{} cópias na pasta", st.total_external).into());
    } else {
        s.set_b_last_external("desligado".into());
        s.set_b_external_sub("Escolha uma pasta e ative abaixo".into());
    }
    if sync_form {
        s.set_b_folder(st.external_folder.clone().into());
        s.set_b_enabled(st.enabled);
        s.set_b_keep(st.keep.max(1).to_string().into());
    }
}

fn load_backup(ctx: &AppCtx, sync_form: bool) {
    let (Some(ui), Some(svc)) = (ctx.ui(), services().and_then(|s| s.backup)) else { return };
    let st = svc.status();
    render_backup(&ui, &st, sync_form);
}

fn bind_backup(ctx: &AppCtx, s: &RetaguardaStore) {
    let c = ctx.clone();
    s.on_b_choose_folder(move || {
        let Some(ui) = c.ui() else { return };
        if let Some(path) = rfd::FileDialog::new().set_title("Escolha a pasta do backup externo").pick_folder() {
            store(&ui).set_b_folder(path.display().to_string().into());
        }
    });
    let c = ctx.clone();
    s.on_b_save(move || {
        let Some(ui) = c.ui() else { return };
        let s = store(&ui);
        let Some(svc) = services().and_then(|s| s.backup) else {
            s.set_b_message_ok(false);
            s.set_b_message("Não foi possível localizar o arquivo do banco de dados.".into());
            return;
        };
        s.set_b_saving(true);
        s.set_b_message(SharedString::new());
        let input = BackupSettingsInput {
            external_folder: Some(s.get_b_folder().to_string()),
            enabled: Some(s.get_b_enabled()),
            keep: Some(serde_json::json!(s.get_b_keep().trim().parse::<f64>().unwrap_or(f64::NAN))),
        };
        let result = svc.save_settings(&input);
        s.set_b_saving(false);
        match result {
            Ok(_) => {
                s.set_b_message_ok(true);
                s.set_b_message("Configuração de backup salva.".into());
                load_backup(&c, true);
            }
            Err(e) => {
                s.set_b_message_ok(false);
                s.set_b_message(e.to_string().into());
            }
        }
    });
    let c = ctx.clone();
    s.on_b_run_now(move || {
        let Some(ui) = c.ui() else { return };
        let Some(svc) = services().and_then(|s| s.backup) else { return };
        let s = store(&ui);
        s.set_b_running(true);
        s.set_b_message(SharedString::new());
        let c2 = c.clone();
        c.spawn(async move { svc.run("manual").await }, move |ui, result| {
            let s = store(ui);
            s.set_b_running(false);
            if let Some(err) = result.external_error.clone().or(result.error.clone()) {
                s.set_b_message_ok(false);
                s.set_b_message(err.into());
            } else {
                s.set_b_message_ok(true);
                s.set_b_message(
                    if result.external.is_some() {
                        "Backup feito no computador e na pasta externa."
                    } else {
                        "Backup feito no computador."
                    }
                    .into(),
                );
            }
            load_backup(&c2, false);
        });
    });
}

/// Backup depois do fechamento do caixa (chamado pela área Caixa).
pub fn backup_after_cash_close(ctx: &AppCtx) {
    let Some(svc) = services().and_then(|s| s.backup) else { return };
    let c = ctx.clone();
    ctx.spawn(async move { svc.run("fechamento de caixa").await }, move |_, result| {
        if let Some(e) = result.error {
            tracing::error!("[Backup] Erro após fechar o caixa: {e}");
        }
        load_backup(&c, false);
    });
}

// ---------- Cardápio digital ----------

/// Chips de categoria em linhas (estimativa da largura de cada chip, como o flex-wrap quebraria), pela largura
/// real do cartão que a tela informa em `m-chips-width` (muda com o tamanho da janela e o zoom da fonte).
fn render_categories(ui: &AppWindow) {
    let (cats, hidden) = with_state(|s| (s.categories.clone(), s.hidden.clone()));
    let available = store(ui).get_m_chips_width().max(160.0);
    let mut rows: Vec<Vec<RtMenuCategory>> = Vec::new();
    let mut width = f32::MAX;
    for (id, name) in cats {
        // 8 px por letra em negrito 12 px (com folga) + borda e espaço entre os chips
        let chip = name.chars().count() as f32 * 8.0 + 34.0;
        if width + chip > available {
            rows.push(Vec::new());
            width = 0.0;
        }
        width += chip;
        let hidden = hidden.contains(&id);
        rows.last_mut().expect("linha criada acima").push(RtMenuCategory { id: id.into(), name: name.into(), hidden });
    }
    let rows: Vec<RtMenuRow> = rows.into_iter().map(|cats| RtMenuRow { cats: model(cats) }).collect();
    store(ui).set_m_category_rows(model(rows));
}

fn render_menu(ui: &AppWindow, st: &DigitalMenuStatus, sync_form: bool) {
    let s = store(ui);
    let live = st.enabled && st.last_published_at.is_some() && st.last_error.is_none();
    let attention = st.enabled && st.last_error.is_some();
    s.set_m_badge(if attention { "ATENÇÃO" } else if live { "NO AR" } else { "DESLIGADO" }.into());
    s.set_m_badge_tone(if attention { 2 } else if live { 1 } else { 0 });
    s.set_m_error(if st.enabled { st.last_error.clone().unwrap_or_default() } else { String::new() }.into());
    let url = st.menu_url.clone().filter(|_| st.enabled);
    s.set_m_published(url.is_some());
    s.set_m_url(url.clone().unwrap_or_default().into());
    match url.as_deref().and_then(qr_image) {
        Some(img) => {
            s.set_m_qr(img);
            s.set_m_has_qr(true);
        }
        None => s.set_m_has_qr(false),
    }
    s.set_m_last(date_time_iso(st.last_published_at.as_deref()).into());
    s.set_m_status_enabled(st.enabled);
    s.set_m_title_placeholder(if st.default_title.is_empty() { "Nome do bar".into() } else { st.default_title.clone().into() });
    s.set_m_site_placeholder(st.site_url.clone().into());
    if sync_form {
        s.set_m_enabled(st.enabled);
        s.set_m_title(st.title.clone().into());
        s.set_m_message(st.message.clone().into());
        s.set_m_hide_sold_out(st.hide_sold_out);
        s.set_m_ignore_stock(st.ignore_stock);
        s.set_m_site(st.custom_site_url.clone().into());
        with_state(|x| x.hidden = st.hidden_category_ids.clone());
    }
    with_state(|x| x.menu = Some(st.clone()));
    render_categories(ui);
}

fn load_menu(ctx: &AppCtx, sync_form: bool) {
    let Some(svc) = services() else { return };
    let db = ctx.db.clone();
    ctx.spawn(
        async move {
            let st = svc.menu.status().await.map_err(|e| e.to_string())?;
            let mut cats = barerp_db::catalog::categories(&db).await.map_err(|e| e.to_string())?;
            cats.sort_by_key(|c| c.sort_order);
            Ok::<_, String>((st, cats))
        },
        move |ui, result| match result {
            Ok((st, cats)) => {
                with_state(|s| s.categories = cats.into_iter().map(|c| (c.id, c.name)).collect());
                render_menu(ui, &st, sync_form);
            }
            Err(e) => tracing::warn!("cardápio digital: {e}"),
        },
    );
}

fn menu_feedback(ui: &AppWindow, ok: bool, text: impl Into<SharedString>) {
    let s = store(ui);
    s.set_m_feedback_ok(ok);
    s.set_m_feedback(text.into());
}

fn bind_menu(ctx: &AppCtx, s: &RetaguardaStore) {
    let c = ctx.clone();
    s.on_m_toggle_category(move |id| {
        let Some(ui) = c.ui() else { return };
        let id = id.to_string();
        with_state(|s| {
            if let Some(pos) = s.hidden.iter().position(|h| *h == id) {
                s.hidden.remove(pos);
            } else {
                s.hidden.push(id);
            }
        });
        render_categories(&ui);
    });
    let c = ctx.clone();
    s.on_m_open_items(move || {
        if let Some(ui) = c.ui() {
            ui.set_page(Page::Products);
            ui.global::<CardapioStore>().invoke_open_menu_items();
        }
    });
    let c = ctx.clone();
    s.on_m_open_menu(move || {
        if let Some(ui) = c.ui() {
            let url = store(&ui).get_m_url();
            if !url.is_empty() {
                open_external(&url);
            }
        }
    });
    let c = ctx.clone();
    s.on_m_copy_link(move || {
        let Some(ui) = c.ui() else { return };
        let url = store(&ui).get_m_url();
        if url.is_empty() {
            return;
        }
        crate::ctx::copy_text(&url);
        menu_feedback(&ui, true, "Link do cardápio copiado.");
    });
    let c = ctx.clone();
    s.on_m_qr_sheet(move || {
        let Some(ui) = c.ui() else { return };
        let Some(st) = with_state(|s| s.menu.clone()) else { return };
        let Some(menu_url) = st.menu_url.clone() else { return };
        let title = if st.title.is_empty() { st.default_title.clone() } else { st.title.clone() };
        store(&ui).set_m_busy("qr".into());
        store(&ui).set_m_feedback(SharedString::new());
        let db = c.db.clone();
        c.spawn(
            async move {
                let tables = barerp_db::tables::list_cards(&db).await.map_err(|e| e.to_string())?;
                let list: Vec<barerp_report::table_qr::QrTable> = tables
                    .iter()
                    .map(|t| barerp_report::table_qr::QrTable { number: t.number, name: t.name.clone() })
                    .collect();
                let pdf = barerp_report::table_qr::table_qr_sheet_pdf(&menu_url, &title, &list).map_err(|e| e.to_string())?;
                Ok::<_, String>((pdf, list.len()))
            },
            |ui, result| {
                store(ui).set_m_busy(SharedString::new());
                match result {
                    Ok((pdf, count)) => {
                        let Some(path) = rfd::FileDialog::new()
                            .set_title("Salvar QR Codes das mesas")
                            .set_file_name(barerp_report::table_qr::TABLE_QR_FILE_NAME)
                            .add_filter("PDF", &["pdf"])
                            .save_file()
                        else {
                            return;
                        };
                        match std::fs::write(&path, pdf) {
                            Ok(()) => {
                                open_external(&path.display().to_string());
                                menu_feedback(ui, true, format!("QR Codes gerados: {count} mesas + 1 geral (balcão/entrada)."));
                            }
                            Err(e) => menu_feedback(ui, false, format!("Não foi possível salvar o PDF: {e}")),
                        }
                    }
                    Err(e) => menu_feedback(ui, false, if e.is_empty() { "Erro ao gerar os QR Codes.".into() } else { e }),
                }
            },
        );
    });
    let c = ctx.clone();
    s.on_m_save(move || {
        let (Some(ui), Some(svc)) = (c.ui(), services()) else { return };
        let s = store(&ui);
        s.set_m_busy("save".into());
        s.set_m_feedback(SharedString::new());
        let hidden = with_state(|x| x.hidden.clone());
        let body = serde_json::json!({
            "enabled": s.get_m_enabled(),
            "title": s.get_m_title().to_string(),
            "message": s.get_m_message().to_string(),
            "hiddenCategoryIds": hidden,
            "hideSoldOut": s.get_m_hide_sold_out(),
            "ignoreStock": s.get_m_ignore_stock(),
            "siteUrl": s.get_m_site().to_string(),
        });
        c.spawn(async move { svc.menu.save_settings(&body).await }, |ui, result| {
            store(ui).set_m_busy(SharedString::new());
            match result {
                Ok(res) => {
                    render_menu(ui, &res.status, true);
                    match res.publish_error {
                        Some(e) => menu_feedback(ui, false, format!("Configuração salva, mas não foi possível publicar: {e}")),
                        None if res.status.enabled => menu_feedback(ui, true, "Cardápio salvo e publicado."),
                        None => menu_feedback(ui, true, "Cardápio digital desligado e retirado do ar."),
                    }
                }
                Err(e) => {
                    let text = e.to_string();
                    menu_feedback(ui, false, if text.is_empty() { "Erro ao salvar o cardápio digital.".into() } else { text });
                }
            }
        });
    });
    let c = ctx.clone();
    s.on_m_chips_relayout(move || {
        if let Some(ui) = c.ui() {
            render_categories(&ui);
        }
    });
    let c = ctx.clone();
    s.on_m_publish(move || {
        let (Some(ui), Some(svc)) = (c.ui(), services()) else { return };
        store(&ui).set_m_busy("publish".into());
        store(&ui).set_m_feedback(SharedString::new());
        let c2 = c.clone();
        c.spawn(
            async move {
                svc.menu.publish(true).await.map_err(|e| e.to_string())?;
                svc.menu.status().await.map_err(|e| e.to_string())
            },
            move |ui, result| {
                store(ui).set_m_busy(SharedString::new());
                match result {
                    Ok(st) => {
                        render_menu(ui, &st, false);
                        menu_feedback(ui, true, "Cardápio publicado. Os clientes já veem a versão atual.");
                    }
                    Err(e) => {
                        menu_feedback(ui, false, if e.is_empty() { "Erro ao publicar o cardápio.".into() } else { e });
                        load_menu(&c2, false);
                    }
                }
            },
        );
    });
}

// ======================================================================
// Dashboard e auditoria
// ======================================================================

fn render_dashboard(ui: &AppWindow, day: NaiveDate, d: &DashboardData) {
    let s = store(ui);
    let today = Local::now().date_naive();
    let label = if day == today {
        "Hoje".to_string()
    } else {
        format!("{}, {}", WEEKDAYS[day.weekday().num_days_from_monday() as usize], day.format("%d/%m"))
    };
    let lower = if day == today { "hoje".to_string() } else { label.clone() };
    s.set_d_date_label(label.into());
    s.set_d_date_text(day.format("%d/%m/%Y").to_string().into());
    s.set_d_is_today(day >= today);

    s.set_d_revenue(brl(d.today_revenue).into());
    s.set_d_orders(d.orders_completed_today.to_string().into());
    s.set_d_ticket(brl(d.average_ticket).into());
    s.set_d_occupancy(format!("{}%", d.occupancy_rate).into());
    s.set_d_occupancy_sub(format!("{} de {} mesas ocupadas", d.occupied_tables, d.total_tables).into());

    let max_qty = d.top_products.first().map(|p| p.quantity).unwrap_or(1).max(1) as f32;
    let top = d
        .top_products
        .iter()
        .enumerate()
        .map(|(i, p)| RtTopProduct {
            rank: i as i32 + 1,
            name: p.name.clone().into(),
            station: if p.station == "BAR" { "🍺 Bar" } else { "🍳 Cozinha" }.into(),
            value: format!("{} un • {}", p.quantity, brl(p.total)).into(),
            pct: (p.quantity as f32 / max_qty * 100.0).round(),
        })
        .collect();
    s.set_d_top(model(top));

    let total = if d.today_revenue > 0.0 { d.today_revenue } else { 1.0 };
    let methods = d
        .payment_methods_breakdown
        .iter()
        .map(|(m, amt)| {
            let pct = (amt / total * 100.0).round();
            RtBar { label: method_label(m).into(), value: format!("{} ({pct}%)", brl(*amt)).into(), pct: pct as f32 }
        })
        .collect();
    s.set_d_methods(model(methods));

    let max_hour = d.sales_by_hour.iter().map(|h| h.total).fold(1.0f64, f64::max);
    let hours: Vec<RtBar> = if d.sales_by_hour.iter().any(|h| h.total > 0.0) {
        d.sales_by_hour
            .iter()
            .map(|h| RtBar {
                label: h.hour.clone().into(),
                value: format!("R$ {:.0}", h.total).into(),
                pct: (h.total / max_hour * 100.0).round() as f32,
            })
            .collect()
    } else {
        Vec::new()
    };
    s.set_d_hours(model(hours));

    let waiters = d
        .waiter_performance
        .iter()
        .enumerate()
        .map(|(i, w)| RtWaiter {
            rank: i as i32 + 1,
            name: w.name.clone().into(),
            sub: format!("{} pedidos fechados {lower}", w.count).into(),
            total: brl(w.total).into(),
        })
        .collect();
    s.set_d_waiters(model(waiters));

    let low = d
        .low_margin_products
        .iter()
        .map(|p| RtLowMargin { name: p.name.clone().into(), margin: format!("{}% Lucro", p.margin).into() })
        .collect();
    s.set_d_low(model(low));
}

fn load_dashboard(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let day = with_state(|s| s.day);
    store(&ui).set_d_loading(true);
    let db = ctx.db.clone();
    ctx.spawn(async move { dashboard::dashboard(&db, day).await }, move |ui, result| {
        store(ui).set_d_loading(false);
        // Resposta de um dia que já não está na tela (cliques rápidos nas setas)
        if with_state(|s| s.day) != day {
            return;
        }
        match result {
            Ok(d) => render_dashboard(ui, day, &d),
            Err(e) => tracing::warn!("Erro ao carregar dashboard: {e}"),
        }
    });
}

fn load_audit(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    store(&ui).set_a_loading(true);
    let db = ctx.db.clone();
    ctx.spawn(async move { dashboard::audit_logs(&db).await }, |ui, result| {
        let s = store(ui);
        s.set_a_loading(false);
        match result {
            Ok(logs) => {
                let rows = logs
                    .into_iter()
                    .map(|l| RtAudit {
                        when: l.created_at.0.with_timezone(&Local).format("%d/%m/%Y, %H:%M:%S").to_string().into(),
                        tone: match l.action.as_str() {
                            "CANCEL_ITEM" => 1,
                            "APPLY_DISCOUNT" => 2,
                            _ => 0,
                        },
                        action: l.action.into(),
                        description: l.description.into(),
                    })
                    .collect();
                s.set_a_logs(model(rows));
            }
            Err(e) => tracing::warn!("Erro ao listar audit logs: {e}"),
        }
    });
}

// ======================================================================
// Impressoras
// ======================================================================

fn render_printers(ui: &AppWindow, p: &PrinterSettings) {
    let s = store(ui);
    s.set_p_cashier(p.cashier_printer.clone().unwrap_or_default().into());
    s.set_p_kitchen(p.kitchen_printer.clone().unwrap_or_default().into());
    s.set_p_paper(p.paper_width.round() as i32);
    s.set_p_margin_left(p.margin_left as f32);
    s.set_p_margin_right(p.margin_right as f32);
    s.set_p_margin_top(p.margin_top as f32);
    s.set_p_margin_bottom(p.margin_bottom as f32);
    s.set_p_qr(p.qr_size.round() as i32);
    s.set_p_font(p.font_scale.round() as i32);
    s.set_p_auto_cut(p.auto_cut);
    s.set_p_silent(p.silent_print);
}

/// Configurações da tela (ainda sem salvar), em cima das que vieram do banco (cópias, avanço, logo).
fn printers_from_form(ui: &AppWindow) -> PrinterSettings {
    let s = store(ui);
    let base = with_state(|x| x.printer.clone());
    let text = |v: SharedString| Some(v.trim().to_string()).filter(|t| !t.is_empty());
    PrinterSettings {
        cashier_printer: text(s.get_p_cashier()),
        kitchen_printer: text(s.get_p_kitchen()),
        paper_width: s.get_p_paper() as f64,
        margin_left: s.get_p_margin_left() as f64,
        margin_right: s.get_p_margin_right() as f64,
        margin_top: s.get_p_margin_top() as f64,
        margin_bottom: s.get_p_margin_bottom() as f64,
        qr_size: s.get_p_qr() as f64,
        font_scale: s.get_p_font() as f64,
        auto_cut: s.get_p_auto_cut(),
        silent_print: s.get_p_silent(),
        ..base
    }
}

fn load_printers(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store(&ui);
    s.set_p_loading(true);
    s.set_p_message(SharedString::new());
    let db = ctx.db.clone();
    ctx.spawn(
        async move {
            let settings = printer::get_printer_settings(&db).await;
            // Impressoras instaladas no sistema (spooler do Windows ou CUPS no macOS/Linux) e a padrão
            let detected = tokio::task::spawn_blocking(|| (barerp_print::installed_printers(), barerp_print::default_printer()))
                .await
                .unwrap_or_default();
            (settings, detected)
        },
        |ui, (settings, (detected, default))| {
            let s = store(ui);
            s.set_p_loading(false);
            s.set_p_detected(model(detected.into_iter().map(SharedString::from).collect()));
            s.set_p_default_printer(default.unwrap_or_default().into());
            match settings {
                Ok(p) => {
                    render_printers(ui, &p);
                    with_state(|x| x.printer = p);
                }
                Err(e) => {
                    s.set_p_message_ok(false);
                    s.set_p_message(format!("Erro ao buscar configurações de impressora: {e}").into());
                }
            }
        },
    );
}

fn bind_printers(ctx: &AppCtx, s: &RetaguardaStore) {
    let c = ctx.clone();
    s.on_p_reload(move || load_printers(&c));
    let c = ctx.clone();
    s.on_p_save(move || {
        let Some(ui) = c.ui() else { return };
        let settings = printers_from_form(&ui);
        store(&ui).set_p_saving(true);
        store(&ui).set_p_saved(false);
        let db = c.db.clone();
        c.spawn(async move { printer::save_printer_settings(&db, &settings).await }, |ui, result| {
            let s = store(ui);
            s.set_p_saving(false);
            match result {
                Ok(saved) => {
                    with_state(|x| x.printer = saved);
                    s.set_p_saved(true);
                    s.set_p_message(SharedString::new());
                    let w = ui.as_weak();
                    later(Duration::from_secs(3), move || {
                        if let Some(ui) = w.upgrade() {
                            store(&ui).set_p_saved(false);
                        }
                    });
                }
                Err(e) => alert(ui, format!("Erro ao salvar: {e}")),
            }
        });
    });
    let c = ctx.clone();
    s.on_p_reset(move || {
        if let Some(ui) = c.ui() {
            let s = store(&ui);
            s.set_confirm_action("printers-reset".into());
            s.set_confirm_text(
                "Deseja restaurar as configurações recomendadas padrão (80mm, margens zeradas e QR Code 100px)?".into(),
            );
        }
    });
    let c = ctx.clone();
    s.on_p_test(move || {
        let Some(ui) = c.ui() else { return };
        let settings = printers_from_form(&ui);
        store(&ui).set_p_testing(true);
        c.spawn(
            async move { tokio::task::spawn_blocking(move || crate::print::print_test_page(&settings)).await.unwrap_or_else(|e| Err(e.to_string())) },
            |ui, result| {
                let s = store(ui);
                s.set_p_testing(false);
                match result {
                    Ok(msg) => {
                        s.set_p_message_ok(true);
                        s.set_p_message(msg.into());
                    }
                    Err(e) => {
                        s.set_p_message_ok(false);
                        s.set_p_message(e.into());
                    }
                }
            },
        );
    });
}

// ======================================================================
// Ligação
// ======================================================================

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store(&ui);
    s.set_version(env!("CARGO_PKG_VERSION").into());

    // Serviços de fundo: backup (precisa do arquivo do banco) e cardápio digital
    {
        // Banco em memória (testes) não tem arquivo: sem backup
        let backup = ctx.rt.block_on(BackupService::from_pool(ctx.db.pool().clone())).ok();
        let _guard = ctx.rt.enter();
        if let Some(b) = &backup {
            b.spawn_scheduler();
        }
        let menu = MenuPublisher::new(DbMenuStore::new(ctx.db.clone()));
        menu.spawn_scheduler();
        // As rotas /api/system do servidor (modo admin do APK) usam os mesmos serviços
        if let Some(server) = &ctx.server {
            server.set_system_services(barerp_server::SystemServices { backup: backup.clone(), menu: menu.clone() });
        }
        SERVICES.with(|x| *x.borrow_mut() = Some(Services { backup, menu }));
    }

    // Navegação
    let c = ctx.clone();
    s.on_shown(move || {
        if let Some(ui) = c.ui() {
            if store(&ui).get_view() == RtView::Hub {
                load_hub(&c);
            }
        }
    });
    let c = ctx.clone();
    s.on_back(move || {
        if let Some(ui) = c.ui() {
            store(&ui).set_view(RtView::Hub);
            load_hub(&c);
        }
    });
    let c = ctx.clone();
    s.on_open_subscription(move || {
        if let Some(ui) = c.ui() {
            store(&ui).set_view(RtView::Subscription);
            refresh_license_quiet(&c);
        }
    });
    let c = ctx.clone();
    s.on_open_dashboard(move || {
        if let Some(ui) = c.ui() {
            store(&ui).set_d_audit_tab(false);
            store(&ui).set_view(RtView::Dashboard);
            with_state(|s| s.day = Local::now().date_naive());
            load_dashboard(&c);
        }
    });
    let c = ctx.clone();
    s.on_open_printers(move || {
        if let Some(ui) = c.ui() {
            store(&ui).set_view(RtView::Printers);
            load_printers(&c);
        }
    });
    let c = ctx.clone();
    s.on_open_staff(move || {
        if let Some(ui) = c.ui() {
            ui.global::<MesasStore>().invoke_open_staff();
        }
    });
    let c = ctx.clone();
    s.on_open_waiters(move || {
        if let Some(ui) = c.ui() {
            ui.global::<MesasStore>().invoke_open_waiters();
        }
    });
    let c = ctx.clone();
    s.on_open_connect(move || {
        if let Some(ui) = c.ui() {
            ui.global::<MesasStore>().invoke_open_connect_mobile();
        }
    });
    // Clientes (Fiado) e Fornecedores abrem dentro da Retaguarda, com as mesmas telas da área Cardápio
    let c = ctx.clone();
    s.on_open_customers(move || {
        if let Some(ui) = c.ui() {
            store(&ui).set_view(RtView::Customers);
            crate::pages::cardapio::people::open_customers(&c);
        }
    });
    let c = ctx.clone();
    s.on_open_suppliers(move || {
        if let Some(ui) = c.ui() {
            store(&ui).set_view(RtView::Suppliers);
            crate::pages::cardapio::people::open_suppliers(&c);
        }
    });

    // Aparência e auto-impressão (preferências deste PC)
    let c = ctx.clone();
    s.on_toggle_theme(move || {
        if let Some(ui) = c.ui() {
            ui.invoke_toggle_theme();
        }
    });
    let c = ctx.clone();
    s.on_cycle_font(move || {
        if let Some(ui) = c.ui() {
            ui.invoke_cycle_font_scale();
        }
    });
    let c = ctx.clone();
    s.on_toggle_auto_print(move || {
        let mut prefs = Preferences::load();
        prefs.auto_print_kitchen = !prefs.auto_print_kitchen;
        prefs.save();
        if let Some(ui) = c.ui() {
            store(&ui).set_auto_print_kitchen(prefs.auto_print_kitchen);
        }
    });

    let c = ctx.clone();
    s.on_save_margin(move || save_margin(&c));

    // Atualização do app (src/pages/update.rs)
    let c = ctx.clone();
    s.on_check_updates(move || crate::pages::update::check_now(&c));
    let c = ctx.clone();
    s.on_open_update(move || {
        if let Some(ui) = c.ui() {
            ui.global::<crate::UpdateStore>().set_modal_open(true);
        }
    });

    // Avisos e confirmações
    let c = ctx.clone();
    s.on_confirm_accepted(move |action| {
        let Some(ui) = c.ui() else { return };
        if action == "printers-reset" {
            let defaults = PrinterSettings::default();
            render_printers(&ui, &PrinterSettings {
                cashier_printer: Some(store(&ui).get_p_cashier().to_string()),
                kitchen_printer: Some(store(&ui).get_p_kitchen().to_string()),
                ..defaults
            });
        }
    });

    // Dashboard
    let c = ctx.clone();
    s.on_d_refresh(move || load_dashboard(&c));
    let c = ctx.clone();
    s.on_d_shift_day(move |delta| {
        let today = Local::now().date_naive();
        with_state(|s| {
            let next = s.day + chrono::Duration::days(delta as i64);
            s.day = next.min(today);
        });
        load_dashboard(&c);
    });
    let c = ctx.clone();
    s.on_d_today(move || {
        with_state(|s| s.day = Local::now().date_naive());
        load_dashboard(&c);
    });
    let c = ctx.clone();
    s.on_a_refresh(move || load_audit(&c));

    bind_license(ctx, &s);
    bind_backup(ctx, &s);
    bind_menu(ctx, &s);
    bind_printers(ctx, &s);

    // ID da máquina (o mesmo do Node) e primeira checagem da licença
    let c = ctx.clone();
    ctx.spawn(
        async move { tokio::task::spawn_blocking(barerp_license::machine::machine_id).await.unwrap_or_else(|_| "unknown-machine".into()) },
        move |_, mid| {
            with_state(|s| s.machine_id = Some(mid));
            refresh_license(&c);
        },
    );
    // Verificação periódica de licença a cada 60 segundos (App.tsx)
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(60), {
        let c = ctx.clone();
        move || refresh_license(&c)
    });
    TIMERS.with(|t| t.borrow_mut().push(timer));

    // Eventos do servidor: cadastro mudou (publica o cardápio), aparelhos conectados, pedidos novos (cozinha)
    if let Some(server) = &ctx.server {
        let mut events = server.subscribe();
        let c = ctx.clone();
        let menu = services().map(|s| s.menu);
        let db = ctx.db.clone();
        ctx.rt.spawn(async move {
            loop {
                let ev = match events.recv().await {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                match ev.name.as_str() {
                    "catalog:updated" => {
                        if let Some(menu) = &menu {
                            menu.schedule_publish();
                        }
                    }
                    "devices_updated" => {
                        let c2 = c.clone();
                        let _ = slint::invoke_from_event_loop(move || refresh_devices(&c2));
                    }
                    // Auto-Impressão de Cozinha ligada neste PC: imprime a comanda do pedido novo
                    "kds:new_order" if Preferences::load().auto_print_kitchen => {
                        if let Err(e) = crate::print::print_kitchen_payload(&db, &ev.payload).await {
                            let _ = c.ui.upgrade_in_event_loop(move |ui| {
                                ui.set_status_message(format!("Falha na comanda da cozinha: {e}").into());
                                ui.set_status_error(true);
                            });
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}

/// Para os prints das telas (`examples/retaguarda_shots.rs`), sem serviços de fundo.
pub mod preview {
    use super::*;

    pub fn hub(ui: &AppWindow, license: &LicenseStatus, menu: &DigitalMenuStatus, categories: Vec<(String, String)>, backup: &BackupStatus) {
        with_state(|s| {
            s.machine_id = Some(license.machine_id.clone());
            s.categories = categories;
        });
        apply_license(ui, license);
        render_menu(ui, menu, true);
        render_backup(ui, backup, true);
        store(ui).set_view(RtView::Hub);
    }

    pub fn dashboard(ui: &AppWindow, day: NaiveDate, data: &DashboardData) {
        with_state(|s| s.day = day);
        render_dashboard(ui, day, data);
        store(ui).set_d_audit_tab(false);
        store(ui).set_view(RtView::Dashboard);
    }

    pub fn audit(ui: &AppWindow, rows: Vec<RtAudit>) {
        store(ui).set_a_logs(model(rows));
        store(ui).set_d_audit_tab(true);
        store(ui).set_view(RtView::Dashboard);
    }

    pub fn printers(ui: &AppWindow, settings: &PrinterSettings) {
        render_printers(ui, settings);
        store(ui).set_view(RtView::Printers);
    }

    pub fn subscription(ui: &AppWindow) {
        store(ui).set_view(RtView::Subscription);
    }

    pub fn gate(ui: &AppWindow, license: &LicenseStatus) {
        apply_license(ui, license);
    }
}
