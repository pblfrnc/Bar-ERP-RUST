//! Prints da venda rápida do Caixa sem abrir janela, em tema escuro e claro:
//! `cargo run -p barerp-app --example caixa_shots -- <cópia do banco> <pasta>`.
//! Aplica as migrações no banco — use sempre uma cópia (ou o banco do `barerp-db --example demo_db`).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

use barerp_app::ctx::AppCtx;
use barerp_app::pages::caixa::preview;
use barerp_app::{AppWindow, Page, Theme};
use barerp_db::{caixa, catalog, Db};

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
    let products = rt.block_on(caixa::pdv_products(&db))?;
    let categories = rt.block_on(catalog::categories(&db))?;
    let top = rt.block_on(caixa::top_sellers(&db, 5000, 15))?;
    if products.len() < 3 {
        return Err("o banco de teste precisa de pelo menos 3 produtos".into());
    }

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(SIZE.0, SIZE.1));
    slint::platform::set_platform(Box::new(OffscreenPlatform { window: window.clone() }))?;

    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_user_name("Administrador".into());
    ui.set_user_role("ADMIN".into());
    ui.set_compact(SIZE.0 < 1500);
    ui.set_tiny(SIZE.0 < 1180);
    ui.set_page(Page::Cash);
    ui.show()?;
    let ctx = AppCtx::new(&ui, db, rt.handle().clone(), None);

    for (dark, suffix) in [(true, "escuro"), (false, "claro")] {
        ui.global::<Theme>().set_dark(dark);
        preview::quick_sale(&ctx, products.clone(), categories.clone(), top.clone(), &[(0, 4), (1, 1), (2, 2)], "CASH", "100", true);
        save(&window, &out.join(format!("x1-venda-rapida-{suffix}.png")))?;
        ui.set_help_open(true);
        save(&window, &out.join(format!("x2-atalhos-{suffix}.png")))?;
        ui.set_help_open(false);
        ui.set_locked(true);
        save(&window, &out.join(format!("x3-tela-bloqueada-{suffix}.png")))?;
        ui.set_locked(false);
    }
    Ok(())
}
