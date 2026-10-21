//! Prints da tela de KDS sem abrir janela, em tema escuro e claro, com uma fila inventada (não usa banco):
//! `cargo run -p barerp-app --example kds_shots -- <pasta>`.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use chrono::{Duration, Utc};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

use barerp_app::pages::kds::preview;
use barerp_app::{AppWindow, KdsStore, Page, Theme};
use barerp_db::kds::KdsItem;
use barerp_db::DbTime;

const WIDTH: u32 = 1440;
const HEIGHT: u32 = 900;

struct OffscreenPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for OffscreenPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn save(window: &MinimalSoftwareWindow, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut buffer = vec![Rgb8Pixel::default(); (WIDTH * HEIGHT) as usize];
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, WIDTH as usize);
    });
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    encoder.write_header()?.write_image_data(&bytes)?;
    println!("{}", path.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn item(id: &str, order: i64, table: Option<i64>, customer: &str, product: &str, qty: i64, station: &str, status: &str, notes: &str, minutes: i64) -> KdsItem {
    KdsItem {
        id: id.into(),
        order_id: format!("ord-{order}"),
        product_id: format!("p-{id}"),
        product_name: product.into(),
        quantity: qty,
        unit_type: None,
        notes: (!notes.is_empty()).then(|| notes.to_string()),
        modifiers: None,
        kds_status: status.into(),
        kds_station: station.into(),
        added_at: DbTime(Utc::now() - Duration::minutes(minutes)),
        order_number: order,
        table_id: table.map(|n| format!("t-{n}")),
        table_number: table,
        table_name: None,
        waiter_id: None,
        waiter_name: Some("João".into()),
        customer_name: (!customer.is_empty()).then(|| customer.to_string()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).ok_or("informe a pasta de saída")?);
    std::fs::create_dir_all(&out)?;

    let items = vec![
        item("1", 41, Some(7), "Mariana", "Porção de Fritas Grande", 1, "KITCHEN", "PREPARING", "Adicional: Cheddar e Bacon | Sem cebola", 17),
        item("2", 41, Some(7), "Mariana", "Heineken Long Neck", 4, "BAR", "READY", "", 17),
        item("3", 41, Some(7), "Mariana", "Caipirinha de Limão", 2, "BAR", "PENDING", "Com gelo e limão", 16),
        item("4", 42, Some(3), "", "Picanha na Chapa", 1, "KITCHEN", "PENDING", "Ponto: mal passada", 9),
        item("5", 42, Some(3), "", "Coca-Cola 350ml", 2, "BAR", "PENDING", "", 9),
        item("6", 43, Some(12), "Família Souza", "Isca de Peixe", 1, "KITCHEN", "PENDING", "", 3),
        item("7", 44, None, "Balcão", "Pastel de Carne", 3, "KITCHEN", "PREPARING", "", 1),
        item("8", 45, Some(5), "Carlos", "Chopp Brahma 300ml", 6, "BAR", "PENDING", "Bem gelada", 0),
    ];

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    slint::platform::set_platform(Box::new(OffscreenPlatform { window: window.clone() }))?;

    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_user_name("Administrador".into());
    ui.set_user_role("ADMIN".into());
    ui.set_compact(WIDTH < 1500);
    ui.set_tiny(WIDTH < 1180);
    ui.set_page(Page::Kds);
    ui.set_kds_total(items.len() as i32);
    ui.show()?;

    for (dark, suffix) in [(true, "escuro"), (false, "claro")] {
        ui.global::<Theme>().set_dark(dark);
        ui.global::<KdsStore>().set_connected(true);
        preview::show(&ui, items.clone(), 4);
        save(&window, &out.join(format!("k1-fila-{suffix}.png")))?;
        ui.global::<KdsStore>().set_connected(false);
        preview::show(&ui, Vec::new(), 4);
        save(&window, &out.join(format!("k2-vazio-offline-{suffix}.png")))?;
    }
    Ok(())
}
