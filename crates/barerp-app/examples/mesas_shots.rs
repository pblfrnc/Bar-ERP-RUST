//! Prints da área Mesas & Salão sem abrir janela, em tema escuro e claro:
//! `cargo run -p barerp-app --example mesas_shots -- <cópia do banco> <pasta>`.
//! Aplica as migrações no banco — use sempre uma cópia.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

use barerp_app::pages::mesas::preview;
use barerp_app::{AppWindow, MesasStore, Page, Theme};
use barerp_db::{catalog, orders, staff, tables, waiters, Db};

/// Tamanho do print: 1440x900, ou o de `BARERP_SHOT_SIZE=1366x768` para conferir telas de notebook.
static SIZE: std::sync::LazyLock<(u32, u32)> = std::sync::LazyLock::new(|| {
    std::env::var("BARERP_SHOT_SIZE")
        .ok()
        .and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))))
        .unwrap_or((1440, 900))
});

struct OffscreenPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for OffscreenPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn save(window: &MinimalSoftwareWindow, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut buffer = vec![Rgb8Pixel::default(); (SIZE.0 * SIZE.1) as usize];
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, SIZE.0 as usize);
    });
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, SIZE.0, SIZE.1);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    encoder.write_header()?.write_image_data(&bytes)?;
    println!("{}", path.display());
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let db_path = PathBuf::from(args.next().ok_or("informe o caminho do banco")?);
    let out = PathBuf::from(args.next().ok_or("informe a pasta de saída")?);
    std::fs::create_dir_all(&out)?;

    let rt = tokio::runtime::Runtime::new()?;
    let db = rt.block_on(Db::open(&db_path))?;
    let cards = rt.block_on(tables::list_cards(&db))?;
    let products = rt.block_on(catalog::sale_products(&db))?;
    let categories = rt.block_on(catalog::categories(&db))?;
    let promotions = rt.block_on(catalog::promotions(&db))?;
    let busy = cards.iter().find(|c| c.order_id.is_some()).ok_or("o banco de teste precisa de uma mesa ocupada")?.clone();
    let detail = rt.block_on(orders::detail(&db, busy.order_id.as_deref().unwrap()))?;
    let waiter_list = rt.block_on(waiters::list(&db, true))?;
    let staff_list = rt.block_on(staff::list_all(&db))?;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(SIZE.0, SIZE.1));
    slint::platform::set_platform(Box::new(OffscreenPlatform { window: window.clone() }))?;

    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_user_name("Administrador".into());
    ui.set_user_role("ADMIN".into());
    ui.set_compact(SIZE.0 < 1500);
    ui.set_tiny(SIZE.0 < 1180);
    ui.set_page(Page::Tables);
    ui.show()?;

    for (dark, suffix) in [(true, "escuro"), (false, "claro")] {
        ui.global::<Theme>().set_dark(dark);
        preview::map(&ui, cards.clone());
        save(&window, &out.join(format!("m1-mapa-{suffix}.png")))?;
        // Número digitado direto no mapa
        ui.global::<MesasStore>().set_jump("12".into());
        save(&window, &out.join(format!("m1b-mapa-digitando-{suffix}.png")))?;
        ui.global::<MesasStore>().set_jump("".into());
        // Planta do salão (sem posições gravadas: fileiras na ordem do número), no modo editar
        ui.global::<MesasStore>().set_plan_mode(true);
        ui.global::<MesasStore>().set_plan_edit(true);
        save(&window, &out.join(format!("m1c-planta-{suffix}.png")))?;
        ui.global::<MesasStore>().set_plan_mode(false);
        ui.global::<MesasStore>().set_plan_edit(false);
        preview::details(&ui, &busy.id, detail.clone());
        save(&window, &out.join(format!("m2-detalhe-{suffix}.png")))?;
        preview::launch(&ui, &busy.id, products.clone(), categories.clone(), promotions.clone());
        save(&window, &out.join(format!("m3-lancar-{suffix}.png")))?;
        preview::checkout(&ui, &busy.id, detail.clone(), "simple", "PIX", "33,00");
        save(&window, &out.join(format!("m4-conta-dividida-{suffix}.png")))?;
        preview::checkout(&ui, &busy.id, detail.clone(), "items", "CASH", "38,00");
        save(&window, &out.join(format!("m5-conta-itens-dinheiro-{suffix}.png")))?;
        preview::checkout(&ui, &busy.id, detail.clone(), "amount", "CREDIT_TAB", "20,00");
        save(&window, &out.join(format!("m6-conta-haver-fiado-{suffix}.png")))?;
        preview::manage(&ui);
        save(&window, &out.join(format!("m7-editar-locais-{suffix}.png")))?;
        preview::waiters(&ui, waiter_list.clone());
        save(&window, &out.join(format!("m8-garcons-{suffix}.png")))?;
        preview::staff(&ui, staff_list.clone());
        save(&window, &out.join(format!("m9-funcionarios-{suffix}.png")))?;
        preview::connect(&ui);
        save(&window, &out.join(format!("m10-conectar-celular-{suffix}.png")))?;
        barerp_app::pages::mesas::close_preview(&ui);
    }
    Ok(())
}
