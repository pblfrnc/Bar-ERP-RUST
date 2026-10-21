use barerp_report::stock_sheet::{
    stock_sheet_pdf, SheetOrder, StockCategory, StockProduct, StockSheetOptions,
};
use barerp_report::table_qr::{table_qr_sheet_pdf, QrTable};
use chrono::NaiveDateTime;

/// Com `BARERP_REPORT_OUT=<pasta>` os PDFs ficam salvos para conferir no leitor.
fn save(name: &str, bytes: &[u8]) {
    if let Ok(dir) = std::env::var("BARERP_REPORT_OUT") {
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
    }
}

/// `/Count` da árvore de páginas.
fn pages(pdf: &[u8]) -> usize {
    let s = String::from_utf8_lossy(pdf);
    let rest = &s[s.find("/Count ").expect("árvore de páginas") + 7..];
    rest.split_whitespace().next().unwrap().parse().unwrap()
}

fn contains(pdf: &[u8], needle: &[u8]) -> bool {
    pdf.windows(needle.len()).any(|w| w == needle)
}

/// Texto mostrado na página (`Tj`), em WinAnsi: o pdf-writer grava `(ascii)` ou `<HEX>` quando há acento.
fn shows(pdf: &[u8], text: &[u8]) -> bool {
    let literal = [b"(", text, b") Tj"].concat();
    let hex: String = text.iter().map(|b| format!("{b:02X}")).collect();
    contains(pdf, &literal) || contains(pdf, format!("<{hex}> Tj").as_bytes())
}

fn at() -> NaiveDateTime {
    NaiveDateTime::parse_from_str("2026-10-18 21:05", "%Y-%m-%d %H:%M").unwrap()
}

fn catalog() -> (Vec<StockProduct>, Vec<StockCategory>) {
    let cats = vec![
        StockCategory {
            id: "beb".into(),
            name: "Bebidas".into(),
            sort_order: 0,
        },
        StockCategory {
            id: "coz".into(),
            name: "Cozinha".into(),
            sort_order: 1,
        },
        StockCategory {
            id: "sob".into(),
            name: "Sobremesas".into(),
            sort_order: 2,
        },
    ];
    let mut ps = Vec::new();
    for i in 0..70 {
        let (cat, name) = match i % 3 {
            0 => ("beb", format!("Cerveja Pilsen long neck 355ml lote {i:02}")),
            1 => (
                "coz",
                format!("Porção de fritas com cheddar, bacon e cebolinha nº {i:02}"),
            ),
            _ => ("sob", format!("Açaí {i:02}")),
        };
        ps.push(StockProduct {
            code: Some(format!("P-{i:03}")),
            name,
            category_id: cat.into(),
            unit: if i % 3 == 0 {
                "un".into()
            } else {
                "porção".into()
            },
            stock: (i * 7 % 50) as i64,
            track_stock: i % 5 != 0,
            price: 9.9 + i as f64,
            cost_price: Some(4.5 + i as f64 / 2.0),
            has_box_price: i % 3 == 0,
            box_quantity: Some(12),
            box_price: Some(99.0),
        });
    }
    (ps, cats)
}

#[test]
fn balanco_por_categoria_com_custo_pagina_e_acentos() {
    let (ps, cats) = catalog();
    let opts = StockSheetOptions {
        store_name: "Bar do Zé".into(),
        category_id: None,
        order: SheetOrder::Category,
        show_cost: true,
    };
    let pdf = stock_sheet_pdf(&ps, &cats, &opts, at());
    save("balanco-categoria.pdf", &pdf);
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(pages(&pdf) >= 2, "70 produtos não cabem numa página");
    // Texto em WinAnsi: "Balanço" com ç = 0xE7
    assert!(shows(
        &pdf,
        b"Balan\xe7o de estoque e revis\xe3o de pre\xe7os"
    ));
    assert!(contains(&pdf, b"/WinAnsiEncoding"));
    assert!(shows(&pdf, b"CUSTO"));
}

#[test]
fn balanco_alfabetico_uma_categoria_sem_custo() {
    let (ps, cats) = catalog();
    let opts = StockSheetOptions {
        store_name: "Bar do Zé".into(),
        category_id: Some("sob".into()),
        order: SheetOrder::Alpha,
        show_cost: false,
    };
    let pdf = stock_sheet_pdf(&ps, &cats, &opts, at());
    save("balanco-alfabetico.pdf", &pdf);
    assert_eq!(pages(&pdf), 1);
    assert!(!shows(&pdf, b"CUSTO"));
    assert!(shows(&pdf, b"Sobremesas"));
}

#[test]
fn qr_das_mesas_seis_por_pagina() {
    let tables: Vec<QrTable> = (1..=11)
        .rev()
        .map(|n| QrTable {
            number: n,
            name: if n == 3 {
                Some("Varanda".into())
            } else {
                Some(n.to_string())
            },
        })
        .collect();
    let pdf = table_qr_sheet_pdf("https://bar-do-ze.netlify.app/", "Bar do Zé", &tables).unwrap();
    save("qr-mesas.pdf", &pdf);
    // 1 cartão do balcão + 11 mesas = 12 cartões = 2 páginas
    assert_eq!(pages(&pdf), 2);
    assert!(shows(&pdf, b"CARD\xc1PIO"));
    assert!(shows(&pdf, b"Aponte a c\xe2mera do celular"));
    assert!(shows(&pdf, b"Varanda"));
}
