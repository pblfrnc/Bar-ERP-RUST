//! Prints do Módulo Fiscal (menu lateral e Painel fiscal) sem abrir janela, em tema escuro e claro:
//! `cargo run -p barerp-app --example fiscal_shots -- <pasta>` (dados inventados).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};

use chrono::{Duration, Utc};

use barerp_app::pages::fiscal::preview;
use barerp_app::{AppWindow, FiscalStore, FxItemNfce, FxPendente, Page, Theme};
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

        // Notas pendentes: uma em cada situação, com e sem XML
        let chave = "1526 1036 2751 6300 0124 5500 2000 0000 0217 9443 1341";
        let pendente = |emitente: &str, nf: &str, valor: &str, estado: i32, xml: bool| FxPendente {
            chave: format!("{emitente}{nf}").into(),
            chave_fmt: chave.into(),
            emitente: emitente.into(),
            cnpj: "07.526.557/0001-00".into(),
            nf: format!("NF {nf} • Série 1").into(),
            data: "21/10/2026".into(),
            valor: valor.into(),
            estado,
            xml,
        };
        let store = ui.global::<FiscalStore>();
        store.set_pd_notas(std::rc::Rc::new(slint::VecModel::from(vec![
            pendente("AMBEV S.A.", "88231", "R$ 4.912,30", 0, true),
            pendente("DISTRIBUIDORA NORTE LTDA", "5104", "R$ 1.208,00", 0, false),
            pendente("FRIGORIFICO PARA LTDA", "22017", "R$ 860,40", 1, true),
            pendente("COMERCIAL DE BEBIDAS BELEM", "1190", "R$ 2.150,00", 2, true),
        ]))
        .into());
        store.set_pd_total(4);
        store.set_pd_pendentes(2);
        store.set_pd_recebidas(1);
        store.set_pd_finalizadas(1);
        store.set_pd_com_xml(3);
        store.set_pd_sem_xml(1);
        store.set_pd_status("todos".into());
        store.set_pd_xml("todos".into());
        preview::painel(&ui, &painel, "pendentes");
        save(&window, &out.join(format!("f3-pendentes-{suffix}.png")))?;

        // Emissão: três itens, um deles sem NCM
        let item = |nome: &str, codigo: &str, ncm: &str, qtd: i32, unitario: &str, subtotal: &str| FxItemNfce {
            nome: nome.into(),
            codigo: codigo.into(),
            ncm_ok: !ncm.is_empty(),
            ncm: ncm.into(),
            cfop: "5102".into(),
            qtd,
            unitario: unitario.into(),
            subtotal: subtotal.into(),
        };
        store.set_nf_itens(std::rc::Rc::new(slint::VecModel::from(vec![
            item("Heineken Long Neck 330ml", "1001", "22030000", 6, "R$ 12,00", "R$ 72,00"),
            item("Porção de Fritas Grande", "3007", "", 1, "R$ 35,00", "R$ 35,00"),
            item("Água Mineral sem Gás 500ml", "4013", "22011000", 2, "R$ 5,00", "R$ 10,00"),
        ]))
        .into());
        store.set_nf_unidades(9);
        store.set_nf_total("R$ 117,00".into());
        preview::painel(&ui, &painel, "nfce");
        save(&window, &out.join(format!("f4-emitir-nfce-{suffix}.png")))?;
        preview::painel(&ui, &painel, "nfe");
        save(&window, &out.join(format!("f5-emitir-nfe-{suffix}.png")))?;
    }
    Ok(())
}
