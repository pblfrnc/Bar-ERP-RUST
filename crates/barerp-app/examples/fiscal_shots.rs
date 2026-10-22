//! Prints do Módulo Fiscal (menu lateral e Painel fiscal) sem abrir janela, em tema escuro e claro:
//! `cargo run -p barerp-app --example fiscal_shots -- <pasta>` (dados inventados).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

use chrono::{Duration, Utc};

use barerp_app::pages::fiscal::preview;
use barerp_app::{AppWindow, Page, Theme};
use barerp_db::notas::{NotaEmitida, NotaRecebidaResumo, Numeracao, PainelFiscal};
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

fn nota(referencia: &str, numero: &str, valor: f64, status: &str, minutos: i64) -> NotaEmitida {
    NotaEmitida {
        id: referencia.into(),
        referencia: referencia.into(),
        chave: None,
        numero: Some(numero.into()),
        serie: Some("2".into()),
        data_emissao: None,
        valor_total: Some(valor),
        status: status.into(),
        xml_url: None,
        pdf_url: None,
        created_at: DbTime(Utc::now() - Duration::minutes(minutos)),
    }
}

fn recebida(emitente: &str, numero: &str, valor: f64, xml: bool) -> NotaRecebidaResumo {
    NotaRecebidaResumo {
        id: numero.into(),
        chave: String::new(),
        emitente: emitente.into(),
        cnpj_emitente: String::new(),
        numero: numero.into(),
        serie: "1".into(),
        data_emissao: String::new(),
        valor_total: valor,
        status: "pendente".into(),
        has_xml: xml,
        created_at: DbTime(Utc::now()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).ok_or("informe a pasta de saída")?);
    std::fs::create_dir_all(&out)?;
    let painel = PainelFiscal {
        nfce_hoje: (42, 3918.4),
        nfe_mes: (6, 12480.0),
        numeracao_nfce: Numeracao { serie: "2".into(), proximo_configurado: 1, ultimo_emitido: Some(1283) },
        numeracao_nfe: Numeracao { serie: "1".into(), proximo_configurado: 118, ultimo_emitido: Some(117) },
        ambiente: "producao".into(),
        rejeitadas: 1,
        processando: 1,
        cancelaveis_nfce: 2,
        cancelaveis_nfe: 0,
        recentes: vec![
            nota("nfce_a", "1283", 83.6, "autorizado", 4),
            nota("nfce_b", "1282", 151.0, "autorizado", 18),
            nota("nfe_c", "117", 2340.0, "processando", 25),
            nota("nfce_d", "1281", 46.0, "erro_autorizacao", 40),
            nota("nfce_e", "1280", 98.6, "autorizado", 55),
            nota("nfce_f", "1279", 32.0, "cancelado", 70),
        ],
        recebidas_pendentes: 3,
        recebidas: vec![
            recebida("Ambev S.A.", "88231", 4912.3, true),
            recebida("Distribuidora Norte", "5104", 1208.0, false),
            recebida("Frigorífico Pará", "22017", 860.4, false),
        ],
    };

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    slint::platform::set_platform(Box::new(OffscreenPlatform { window: window.clone() }))?;

    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_user_name("Administrador".into());
    ui.set_user_role("ADMIN".into());
    ui.set_compact(WIDTH < 1500);
    ui.set_tiny(WIDTH < 1180);
    ui.set_page(Page::Fiscal);
    ui.show()?;

    for (dark, suffix) in [(true, "escuro"), (false, "claro")] {
        ui.global::<Theme>().set_dark(dark);
        preview::painel(&ui, &painel, "hub");
        save(&window, &out.join(format!("f1-painel-fiscal-{suffix}.png")))?;
        preview::painel(&ui, &painel, "cancelar-nfce");
        save(&window, &out.join(format!("f2-cancelar-{suffix}.png")))?;
    }
    Ok(())
}
