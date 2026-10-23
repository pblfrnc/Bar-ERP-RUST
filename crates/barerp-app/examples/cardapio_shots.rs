//! Prints da área Cardápio & Estoque sem abrir janela, em tema escuro e claro:
//! `cargo run -p barerp-app --example cardapio_shots -- <cópia do banco> <pasta>`.
//! Aplica as migrações no banco — use sempre uma cópia.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

use barerp_app::pages::cardapio::preview;
use barerp_app::{AppWindow, CardapioStore, CdModal, CdNcm, Page, Theme};
use barerp_db::{modifiers, products, suppliers, Db};

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
    let list = rt.block_on(products::list(&db))?;
    let categories = rt.block_on(products::categories(&db))?;
    let supplier_list = rt.block_on(suppliers::list(&db, None))?;
    let groups = rt.block_on(modifiers::list(&db))?;
    let report = rt.block_on(products::stock_report(&db))?;
    let first = list.iter().find(|p| p.cost_price.is_some()).or(list.first()).ok_or("o banco de teste precisa de produtos")?;
    let detail = rt.block_on(products::detail(&db, &first.id))?;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(SIZE.0, SIZE.1));
    slint::platform::set_platform(Box::new(OffscreenPlatform { window: window.clone() }))?;

    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_user_name("Administrador".into());
    ui.set_user_role("ADMIN".into());
    ui.set_compact(SIZE.0 < 1500);
    ui.set_tiny(SIZE.0 < 1180);
    ui.set_page(Page::Products);
    ui.show()?;

    for (dark, suffix) in [(true, "escuro"), (false, "claro")] {
        ui.global::<Theme>().set_dark(dark);
        preview::list(&ui, list.clone(), categories.clone(), supplier_list.clone(), groups.clone());
        save(&window, &out.join(format!("c1-lista-{suffix}.png")))?;
        preview::editor(&ui, &detail);
        save(&window, &out.join(format!("c2-cadastro-{suffix}.png")))?;
        preview::list(&ui, list.clone(), categories.clone(), supplier_list.clone(), groups.clone());
        preview::report(&ui, &report);
        save(&window, &out.join(format!("c3-relatorio-{suffix}.png")))?;
        preview::modifiers(&ui);
        save(&window, &out.join(format!("c4-adicionais-{suffix}.png")))?;
        preview::menu_items(&ui);
        save(&window, &out.join(format!("c5-cardapio-digital-{suffix}.png")))?;
        preview::suppliers(&ui);
        save(&window, &out.join(format!("c6-fornecedores-{suffix}.png")))?;
        // Tabela NCM com a pesquisa pelo nome (resultados de exemplo, sem chamar a Focus)
        preview::list(&ui, list.clone(), categories.clone(), supplier_list.clone(), groups.clone());
        let st = ui.global::<CardapioStore>();
        st.set_modal(CdModal::Ncm);
        st.set_n_code("cerveja".into());
        st.set_n_total_label("3 RESULTADOS · TOQUE PARA VER O CÓDIGO".into());
        st.set_n_results(std::rc::Rc::new(slint::VecModel::from(vec![
            CdNcm { codigo: "22030000".into(), descricao: "Cervejas de malte.".into() },
            CdNcm { codigo: "22029900".into(), descricao: "Águas, incluindo as águas minerais e as águas gaseificadas, adicionadas de açúcar ou de outros edulcorantes ou aromatizadas e outras bebidas não alcoólicas. Outras. Outras (cerveja sem álcool)".into() },
            CdNcm { codigo: "22060090".into(), descricao: "Outras bebidas fermentadas. Outras".into() },
        ]))
        .into());
        save(&window, &out.join(format!("c7-ncm-por-nome-{suffix}.png")))?;
        st.set_n_results(std::rc::Rc::new(slint::VecModel::<CdNcm>::default()).into());
        st.set_modal(CdModal::None);
        preview::close(&ui);
    }
    Ok(())
}
