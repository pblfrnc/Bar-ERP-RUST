//! Folha A4 com o QR Code do cardápio digital de cada mesa (6 por página, com linha de recorte).
//! Tradução de `client/src/utils/tableQrSheet.ts`.

use qrcode::{Color, EcLevel, QrCode};

use crate::pdf::{ascent, ellipsize, pt_mm, rgb, text_width, Anchor, Canvas, Font, Rgb};
use crate::ReportError;

#[derive(Debug, Clone, PartialEq)]
pub struct QrTable {
    pub number: i64,
    pub name: Option<String>,
}

/// Nome sugerido ao salvar.
pub const TABLE_QR_FILE_NAME: &str = "QR Codes das mesas.pdf";

/// `https://cardapio/?mesa=7`; sem mesa (ou mesa 0) devolve o endereço do cardápio.
pub fn table_menu_url(menu_url: &str, table_number: Option<i64>) -> String {
    match table_number {
        Some(n) if n != 0 => format!("{menu_url}?mesa={n}"),
        _ => menu_url.to_string(),
    }
}

const INK: Rgb = rgb(0x0c0a09);
const STONE_600: Rgb = rgb(0x57534e);
const STONE_500: Rgb = rgb(0x78716c);
const STONE_400: Rgb = rgb(0xa8a29e);
const AMBER_700: Rgb = rgb(0xb45309);

const MARGIN: f32 = 10.0;
const CARD_W: f32 = 95.0;
const CARD_H: f32 = 91.0;
const PER_PAGE: usize = 6;
const QR_SIZE: f32 = 44.0;

struct Card {
    label: &'static str,
    big: String,
    extra: String,
    url: String,
}

/// PDF com um cartão "Cardápio" (balcão e entrada) e um por mesa, em ordem de número.
pub fn table_qr_sheet_pdf(
    menu_url: &str,
    store_name: &str,
    tables: &[QrTable],
) -> Result<Vec<u8>, ReportError> {
    let mut sorted: Vec<&QrTable> = tables.iter().collect();
    sorted.sort_by_key(|t| t.number);
    // Primeiro cartão sem mesa: para o balcão, a entrada ou as redes sociais
    let mut cards = vec![Card {
        label: "Cardápio",
        big: String::new(),
        extra: "Balcão e entrada".into(),
        url: table_menu_url(menu_url, None),
    }];
    for t in sorted {
        let name = t.name.as_deref().map(str::trim).unwrap_or("");
        let extra = if !name.is_empty() && name != t.number.to_string() {
            name.to_string()
        } else {
            String::new()
        };
        cards.push(Card {
            label: "Mesa",
            big: t.number.to_string(),
            extra,
            url: table_menu_url(menu_url, Some(t.number)),
        });
    }

    let mut canvas = Canvas::new();
    for (i, card) in cards.iter().enumerate() {
        if i > 0 && i % PER_PAGE == 0 {
            canvas.new_page();
        }
        let slot = i % PER_PAGE;
        let x = MARGIN + (slot % 2) as f32 * CARD_W;
        let y = MARGIN + (slot / 2) as f32 * CARD_H;
        draw_card(&mut canvas, x, y, store_name, card)?;
    }
    Ok(canvas.finish("QR Codes das mesas"))
}

fn draw_card(
    canvas: &mut Canvas,
    x: f32,
    y: f32,
    store: &str,
    card: &Card,
) -> Result<(), ReportError> {
    canvas.stroke_rect(x, y, CARD_W, CARD_H, STONE_400, 0.26, Some(1.0));
    let cx = x + CARD_W / 2.0;

    // Alturas das linhas (line-height normal ≈ 1.2)
    let store_h = pt_mm(11.0) * 1.2;
    let label_size = if card.big.is_empty() { 15.0 } else { 26.0 };
    let label_h = 1.0 + pt_mm(label_size) * 1.2;
    let extra_h = if card.extra.is_empty() {
        0.0
    } else {
        pt_mm(9.0) * 1.2
    };
    let qr_h = QR_SIZE + 2.0 * 2.5;
    let hint_h = 2.0 * pt_mm(9.0) * 1.3;
    let total = store_h + label_h + extra_h + qr_h + hint_h;
    let mut top = y + (CARD_H - total) / 2.0;

    let store = ellipsize(Font::Bold, 11.0, store, 80.0);
    let spacing_store = pt_mm(11.0) * 0.02;
    canvas.text(
        cx,
        top + baseline_in(store_h, 11.0),
        Anchor::Center,
        Font::Bold,
        11.0,
        STONE_600,
        &store,
        spacing_store,
    );
    top += store_h;

    // "MESA 7": rótulo e número grande na mesma linha de base, centralizados juntos
    let label = card.label.to_uppercase();
    let label_spacing = pt_mm(15.0) * 0.06;
    let b = top + 1.0 + baseline_in(label_h - 1.0, label_size);
    if card.big.is_empty() {
        canvas.text(
            cx,
            b,
            Anchor::Center,
            Font::Bold,
            15.0,
            INK,
            &label,
            label_spacing,
        );
    } else {
        let lw = text_width(Font::Bold, 15.0, &format!("{label} "), label_spacing);
        let bw = text_width(Font::Bold, 26.0, &card.big, 0.0);
        let start = cx - (lw + bw) / 2.0;
        canvas.text(
            start,
            b,
            Anchor::Left,
            Font::Bold,
            15.0,
            INK,
            &label,
            label_spacing,
        );
        canvas.text(
            start + lw,
            b,
            Anchor::Left,
            Font::Bold,
            26.0,
            AMBER_700,
            &card.big,
            0.0,
        );
    }
    top += label_h;

    if !card.extra.is_empty() {
        let extra = ellipsize(Font::Bold, 9.0, &card.extra, CARD_W - 8.0);
        canvas.text(
            cx,
            top + baseline_in(extra_h, 9.0),
            Anchor::Center,
            Font::Bold,
            9.0,
            STONE_500,
            &extra,
            0.0,
        );
        top += extra_h;
    }

    draw_qr(canvas, cx - QR_SIZE / 2.0, top + 2.5, QR_SIZE, &card.url)?;
    top += qr_h;

    let line = pt_mm(9.0) * 1.3;
    for (i, l) in ["Aponte a câmera do celular", "para ver o cardápio"]
        .iter()
        .enumerate()
    {
        let b = top + i as f32 * line + baseline_in(line, 9.0);
        canvas.text(cx, b, Anchor::Center, Font::Bold, 9.0, STONE_600, l, 0.0);
    }
    Ok(())
}

/// Linha de base de um texto centralizado verticalmente numa linha de altura `h`.
fn baseline_in(h: f32, size: f32) -> f32 {
    (h - pt_mm(size)) / 2.0 + ascent(size)
}

/// QR com correção M e sem margem, como o `qrcode` do Node; módulos pretos vizinhos viram um só retângulo.
fn draw_qr(canvas: &mut Canvas, x: f32, y: f32, size: f32, url: &str) -> Result<(), ReportError> {
    let code = QrCode::with_error_correction_level(url.as_bytes(), EcLevel::M).map_err(|e| {
        ReportError::Qr {
            url: url.to_string(),
            message: e.to_string(),
        }
    })?;
    let n = code.width();
    let colors = code.to_colors();
    let m = size / n as f32;
    for row in 0..n {
        let mut col = 0;
        while col < n {
            if colors[row * n + col] != Color::Dark {
                col += 1;
                continue;
            }
            let start = col;
            while col < n && colors[row * n + col] == Color::Dark {
                col += 1;
            }
            // Um fio a mais na altura evita linhas brancas entre as fileiras em alguns leitores de PDF
            canvas.fill_rect(
                x + start as f32 * m,
                y + row as f32 * m,
                (col - start) as f32 * m,
                m + m * 0.05,
                INK,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_da_mesa() {
        assert_eq!(
            table_menu_url("https://bar.netlify.app/", Some(7)),
            "https://bar.netlify.app/?mesa=7"
        );
        assert_eq!(
            table_menu_url("https://bar.netlify.app/", None),
            "https://bar.netlify.app/"
        );
        assert_eq!(
            table_menu_url("https://bar.netlify.app/", Some(0)),
            "https://bar.netlify.app/"
        );
    }
}
