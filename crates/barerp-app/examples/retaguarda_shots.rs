//! Prints da Retaguarda sem abrir janela, em tema escuro e claro, com dados inventados (não usa banco):
//! `cargo run -p barerp-app --example retaguarda_shots -- <pasta>` (largura da janela: `SHOTS_WIDTH`, padrão 1440;
//! altura: `SHOTS_HEIGHT`, padrão 900; zoom do Windows × "Aumentar Tamanho": `SHOTS_SCALE`, padrão 1).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use chrono::{Duration, Local, Utc};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

use barerp_app::pages::retaguarda::preview;
use barerp_app::{AppWindow, Page, RetaguardaStore, RtAudit, RtDevice, Theme};
use barerp_backup::{BackupFileInfo, BackupStatus};
use barerp_db::dashboard::{DashboardData, HourSales, LowMarginProduct, TopProduct, WaiterPerformance};
use barerp_db::printer::PrinterSettings;
use barerp_license::{DeveloperContact, LicenseStatus};
use barerp_menu_publish::DigitalMenuStatus;

fn height() -> u32 {
    static H: OnceLock<u32> = OnceLock::new();
    *H.get_or_init(|| std::env::var("SHOTS_HEIGHT").ok().and_then(|h| h.parse().ok()).unwrap_or(900))
}

fn scale() -> f32 {
    std::env::var("SHOTS_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0)
}

/// Largura da janela dos prints (1280 e 1366 são as telas comuns dos PCs de caixa).
fn width() -> u32 {
    static W: OnceLock<u32> = OnceLock::new();
    *W.get_or_init(|| std::env::var("SHOTS_WIDTH").ok().and_then(|w| w.parse().ok()).unwrap_or(1440))
}

struct OffscreenPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for OffscreenPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn save(window: &MinimalSoftwareWindow, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut buffer = vec![Rgb8Pixel::default(); (width() * height()) as usize];
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, width() as usize);
    });
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width(), height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    encoder.write_header()?.write_image_data(&bytes)?;
    println!("{}", path.display());
    Ok(())
}

fn license(days: i64, status: &str, licensed: bool) -> LicenseStatus {
    LicenseStatus {
        machine_id: "d1bfaf4aff653cb27984b7d978e51a7d406d1572df95d205c254beb18dc134d3".into(),
        restaurant_name: "JUBAR".into(),
        is_licensed: licensed,
        license_status: status.into(),
        days_remaining: days,
        is_expiring_soon: licensed && days <= 5,
        expires_at: Some(Utc::now() + Duration::days(days)),
        last_verified_at: Some(Utc::now()),
        client_name: Some("JUBAR Bar e Petiscaria".into()),
        license_server_url: None,
        default_profit_margin: 50.0,
        developer_contact: DeveloperContact::default(),
        should_check_remote: false,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).ok_or("informe a pasta de saída")?);
    std::fs::create_dir_all(&out)?;

    let menu = DigitalMenuStatus {
        enabled: true,
        site_url: "https://cardapio-barerp.netlify.app".into(),
        custom_site_url: String::new(),
        store_id: Some("jubar-7f3a".into()),
        menu_url: Some("https://cardapio-barerp.netlify.app/m/jubar-7f3a".into()),
        title: String::new(),
        default_title: "JUBAR".into(),
        message: "Wi-Fi JUBAR · senha 12345678".into(),
        hidden_category_ids: vec!["c3".into()],
        hide_sold_out: false,
        ignore_stock: false,
        last_published_at: Some((Utc::now() - Duration::minutes(12)).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
        last_error: None,
        publishing: false,
    };
    let categories = ["Cervejas", "Destilados", "Drinks", "Porções", "Lanches", "Refrigerantes", "Sobremesas", "Combos"]
        .iter()
        .enumerate()
        .map(|(i, n)| (format!("c{i}"), n.to_string()))
        .collect::<Vec<_>>();
    let at = |h: i64| (Utc::now() - Duration::hours(h)).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let backup = BackupStatus {
        enabled: true,
        external_folder: r"C:\Users\Bar\Google Drive\Backups BarERP".into(),
        keep: 30,
        last_local: Some(BackupFileInfo { file: "backup_2026-10-19_2130.db".into(), at: at(2) }),
        last_external: Some(BackupFileInfo { file: "bar-erp-backup_2026-10-18_2130.db".into(), at: at(30) }),
        external_reachable: true,
        total_local: 14,
        total_external: 9,
        last_error: None,
        last_error_at: None,
        running: false,
    };
    let dash = DashboardData {
        today_revenue: 4382.5,
        orders_completed_today: 37,
        occupied_tables: 9,
        total_tables: 20,
        occupancy_rate: 45,
        average_ticket: 118.45,
        top_products: vec![
            TopProduct { name: "Heineken Long Neck".into(), quantity: 86, total: 1032.0, station: "BAR".into() },
            TopProduct { name: "Porção de Fritas".into(), quantity: 31, total: 1085.0, station: "KITCHEN".into() },
            TopProduct { name: "Caipirinha de Limão".into(), quantity: 24, total: 528.0, station: "BAR".into() },
            TopProduct { name: "Isca de Peixe".into(), quantity: 12, total: 708.0, station: "KITCHEN".into() },
            TopProduct { name: "Coca-Cola 350ml".into(), quantity: 11, total: 77.0, station: "BAR".into() },
        ],
        payment_methods_breakdown: vec![
            ("PIX".into(), 2210.0),
            ("CREDIT_CARD".into(), 1180.5),
            ("CASH".into(), 640.0),
            ("DEBIT_CARD".into(), 352.0),
        ],
        sales_by_hour: (17..=23)
            .map(|h| HourSales { hour: format!("{h}h"), total: [120.0, 340.0, 610.0, 980.0, 1240.0, 760.0, 332.5][(h - 17) as usize] })
            .collect(),
        waiter_performance: vec![
            WaiterPerformance { name: "João".into(), total: 1820.0, count: 14 },
            WaiterPerformance { name: "Mariana".into(), total: 1490.5, count: 12 },
            WaiterPerformance { name: "Pedro".into(), total: 812.0, count: 8 },
            WaiterPerformance { name: "Sem Garçom".into(), total: 260.0, count: 3 },
        ],
        low_margin_products: vec![
            LowMarginProduct { id: "p1".into(), name: "Água Mineral 500ml".into(), margin: 18, price: 4.0, cost: 3.28 },
            LowMarginProduct { id: "p2".into(), name: "Red Bull 250ml".into(), margin: 24, price: 15.0, cost: 11.4 },
        ],
    };
    let audit = vec![
        RtAudit { when: "19/10/2026, 22:41:07".into(), action: "CANCEL_ITEM".into(), description: "Item cancelado: 2x Heineken Long Neck (Comanda #41, Mesa 7) por Administrador".into(), tone: 1 },
        RtAudit { when: "19/10/2026, 21:58:30".into(), action: "APPLY_DISCOUNT".into(), description: "Desconto de R$ 10,00 na Comanda #39 (Mesa 3)".into(), tone: 2 },
        RtAudit { when: "19/10/2026, 20:12:44".into(), action: "LOSS_CALOTE".into(), description: "Perda (Calote/Evasão) de R$ 86,00 na Comanda #33 (Mesa 12)".into(), tone: 0 },
    ];

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(width(), height()));
    slint::platform::set_platform(Box::new(OffscreenPlatform { window: window.clone() }))?;

    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_user_name("Administrador".into());
    ui.set_user_role("ADMIN".into());
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale() });
    let logical = width() as f32 / scale();
    ui.set_compact(logical < 1500.0);
    ui.set_tiny(logical < 1180.0);
    ui.set_page(Page::Settings);
    ui.show()?;
    let store = ui.global::<RetaguardaStore>();
    store.set_version(env!("CARGO_PKG_VERSION").into());
    store.set_devices(slint::ModelRc::new(slint::VecModel::from(vec![
        RtDevice { kind: "GARCOM_MOBILE".into(), label: "📱 App Garçom".into(), waiter: "João".into(), agent: "Mozilla/5.0 (Linux; Android 14; SM-A155M)".into(), ip: "192.168.0.23".into() },
        RtDevice { kind: "COZINHA_KDS".into(), label: "📺 Tela Cozinha (KDS)".into(), waiter: "".into(), agent: "Mozilla/5.0 (Linux; Android 13; Tab A8)".into(), ip: "192.168.0.31".into() },
    ])));

    let day = Local::now().date_naive();
    for (dark, suffix) in [(true, "escuro"), (false, "claro")] {
        ui.global::<Theme>().set_dark(dark);
        store.set_g_visible(false);
        preview::hub(&ui, &license(3, "ACTIVE", true), &menu, categories.clone(), &backup);
        save(&window, &out.join(format!("r1-hub-{suffix}.png")))?;
        preview::dashboard(&ui, day, &dash);
        save(&window, &out.join(format!("r2-dashboard-{suffix}.png")))?;
        preview::audit(&ui, audit.clone());
        save(&window, &out.join(format!("r3-auditoria-{suffix}.png")))?;
        preview::printers(&ui, &PrinterSettings { cashier_printer: Some("POS-80".into()), ..PrinterSettings::default() });
        // As impressoras do notebook em que o Pablo testou
        let printers = ["Wondershare PDFelement", "PDV", "OneNote (Desktop)", "Microsoft Print to PDF", "Canon G3010 series"];
        store.set_p_detected(slint::ModelRc::new(slint::VecModel::from(printers.iter().map(|p| slint::SharedString::from(*p)).collect::<Vec<_>>())));
        store.set_p_default_printer("Canon G3010 series".into());
        save(&window, &out.join(format!("r4-impressoras-{suffix}.png")))?;
        store.set_view(barerp_app::RtView::Customers);
        save(&window, &out.join(format!("r4b-clientes-{suffix}.png")))?;
        preview::subscription(&ui);
        save(&window, &out.join(format!("r5-assinatura-{suffix}.png")))?;
        preview::gate(&ui, &license(0, "EXPIRED", false));
        save(&window, &out.join(format!("r6-licenca-expirada-{suffix}.png")))?;
        store.set_g_visible(false);
        store.set_i_visible(true);
        save(&window, &out.join(format!("r7-aviso-mensalidade-{suffix}.png")))?;
        store.set_i_visible(false);
        let info = barerp_updater::UpdateInfo {
            has_update: true,
            current_version: "2.0.0".into(),
            latest_version: "v2.1.0".into(),
            release_name: "BarERP Pro v2.1.0".into(),
            release_notes: "Nova compilação do BarERP com melhorias de sistema e atualizações.".into(),
            asset_date: Some(Utc::now()),
            download_url: String::new(),
            file_size: 48_000_000,
            message: None,
        };
        barerp_app::pages::update::preview::show(&ui, &info, false);
        preview::subscription(&ui);
        save(&window, &out.join(format!("r8-aviso-atualizacao-{suffix}.png")))?;
        barerp_app::pages::update::preview::show(&ui, &info, true);
        save(&window, &out.join(format!("r9-janela-atualizacao-{suffix}.png")))?;
        barerp_app::pages::update::preview::show(&ui, &barerp_updater::UpdateInfo::default(), false);
    }
    Ok(())
}
