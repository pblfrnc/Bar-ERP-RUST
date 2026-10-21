//! Motor mínimo de página A4: coordenadas em mm a partir do canto superior esquerdo,
//! texto nas fontes padrão (Helvetica, Helvetica-Bold, Courier) e retângulos.

use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use crate::metrics::{HELVETICA, HELVETICA_BOLD};
use crate::text::winansi;

pub(crate) const A4_W: f32 = 210.0;
pub(crate) const A4_H: f32 = 297.0;

fn pt(mm: f32) -> f32 {
    mm * 72.0 / 25.4
}

/// Tamanho em pontos convertido para mm.
pub(crate) fn pt_mm(size: f32) -> f32 {
    size * 25.4 / 72.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Font {
    Regular,
    Bold,
    Mono,
}

impl Font {
    fn resource(self) -> Name<'static> {
        match self {
            Font::Regular => Name(b"F1"),
            Font::Bold => Name(b"F2"),
            Font::Mono => Name(b"F3"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rgb(f32, f32, f32);

/// `0x0c0a09` → cor.
pub(crate) const fn rgb(hex: u32) -> Rgb {
    Rgb(
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
    )
}

/// Largura do texto em mm, com `spacing` mm extras por caractere.
pub(crate) fn text_width(font: Font, size: f32, s: &str, spacing: f32) -> f32 {
    let units: u32 = winansi(s)
        .iter()
        .map(|&b| match font {
            Font::Regular => HELVETICA[b as usize] as u32,
            Font::Bold => HELVETICA_BOLD[b as usize] as u32,
            Font::Mono => 600,
        })
        .sum();
    pt_mm(size * units as f32 / 1000.0) + spacing * s.chars().count() as f32
}

/// Quebra por palavra na largura (palavra maior que a linha é cortada).
pub(crate) fn wrap(font: Font, size: f32, s: &str, max: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text_width(font, size, &candidate, 0.0) <= max {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        // A palavra sozinha ainda pode não caber
        for c in word.chars() {
            let next = format!("{cur}{c}");
            if !cur.is_empty() && text_width(font, size, &next, 0.0) > max {
                lines.push(std::mem::take(&mut cur));
                cur.push(c);
            } else {
                cur = next;
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Corta com reticências para caber na largura (como `text-overflow: ellipsis`).
pub(crate) fn ellipsize(font: Font, size: f32, s: &str, max: f32) -> String {
    if text_width(font, size, s, 0.0) <= max {
        return s.to_string();
    }
    let mut out: String = s.to_string();
    while !out.is_empty() && text_width(font, size, &format!("{out}…"), 0.0) > max {
        out.pop();
    }
    format!("{}…", out.trim_end())
}

/// Distância do topo da linha até a linha de base, em mm.
pub(crate) fn ascent(size: f32) -> f32 {
    pt_mm(size) * 0.78
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor {
    Left,
    Center,
    Right,
}

pub(crate) struct Canvas {
    pages: Vec<Content>,
}

impl Canvas {
    pub fn new() -> Self {
        Self {
            pages: vec![Content::new()],
        }
    }

    pub fn new_page(&mut self) {
        self.pages.push(Content::new());
    }

    fn cur(&mut self) -> &mut Content {
        self.pages.last_mut().expect("sempre há uma página")
    }

    /// Texto com a linha de base em `baseline` (mm a partir do topo).
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        x: f32,
        baseline: f32,
        anchor: Anchor,
        font: Font,
        size: f32,
        color: Rgb,
        s: &str,
        spacing: f32,
    ) {
        let w = text_width(font, size, s, spacing);
        let x = match anchor {
            Anchor::Left => x,
            Anchor::Center => x - w / 2.0,
            Anchor::Right => x - w,
        };
        let bytes = winansi(s);
        let c = self.cur();
        c.set_fill_rgb(color.0, color.1, color.2);
        c.begin_text();
        c.set_font(font.resource(), size);
        // Tc fica no estado gráfico: zera sempre para não vazar para o próximo texto
        c.set_char_spacing(pt(spacing));
        c.next_line(pt(x), pt(A4_H - baseline));
        c.show(Str(&bytes));
        c.end_text();
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        let c = self.cur();
        c.set_fill_rgb(color.0, color.1, color.2);
        c.rect(pt(x), pt(A4_H - y - h), pt(w), pt(h));
        c.fill_nonzero();
    }

    /// Contorno; `dash` em mm para borda tracejada.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgb,
        width: f32,
        dash: Option<f32>,
    ) {
        let c = self.cur();
        c.save_state();
        c.set_stroke_rgb(color.0, color.1, color.2);
        c.set_line_width(pt(width));
        if let Some(d) = dash {
            c.set_dash_pattern([pt(d), pt(d)], 0.0);
        }
        c.rect(pt(x), pt(A4_H - y - h), pt(w), pt(h));
        c.stroke();
        c.restore_state();
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: Rgb, width: f32) {
        let c = self.cur();
        c.set_stroke_rgb(color.0, color.1, color.2);
        c.set_line_width(pt(width));
        c.move_to(pt(x1), pt(A4_H - y1));
        c.line_to(pt(x2), pt(A4_H - y2));
        c.stroke();
    }

    /// Monta o arquivo PDF com todas as páginas.
    pub fn finish(self, title: &str) -> Vec<u8> {
        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let info = Ref::new(3);
        let fonts = [
            (Font::Regular, Ref::new(4), "Helvetica"),
            (Font::Bold, Ref::new(5), "Helvetica-Bold"),
            (Font::Mono, Ref::new(6), "Courier"),
        ];
        let mut next = 7;
        let mut ids = Vec::new();
        for _ in &self.pages {
            ids.push((Ref::new(next), Ref::new(next + 1)));
            next += 2;
        }

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree)
            .kids(ids.iter().map(|(p, _)| *p))
            .count(ids.len() as i32);
        pdf.document_info(info)
            .title(TextStr(title))
            .producer(TextStr("BarERP"));
        for (_, id, base) in fonts {
            pdf.type1_font(id)
                .base_font(Name(base.as_bytes()))
                .encoding_predefined(Name(b"WinAnsiEncoding"));
        }
        for ((page_id, content_id), content) in ids.iter().zip(self.pages) {
            let mut page = pdf.page(*page_id);
            page.media_box(Rect::new(0.0, 0.0, pt(A4_W), pt(A4_H)));
            page.parent(tree);
            page.contents(*content_id);
            let mut res = page.resources();
            let mut f = res.fonts();
            for (font, id, _) in fonts {
                f.pair(font.resource(), id);
            }
            f.finish();
            res.finish();
            page.finish();
            pdf.stream(*content_id, &content.finish());
        }
        pdf.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn largura_helvetica() {
        // "Helvetica" = 722+556+222+500+556+278+222+500+556 = 4112 milésimos
        let w = text_width(Font::Regular, 10.0, "Helvetica", 0.0);
        assert!((w - pt_mm(41.12)).abs() < 1e-4);
        assert!(text_width(Font::Bold, 10.0, "Helvetica", 0.0) > w);
        assert!((text_width(Font::Mono, 10.0, "abc", 0.0) - pt_mm(18.0)).abs() < 1e-4);
    }

    #[test]
    fn quebra_e_reticencias() {
        let lines = wrap(Font::Regular, 10.0, "Cerveja Pilsen long neck 355ml", 25.0);
        assert!(lines.len() > 1);
        assert!(lines
            .iter()
            .all(|l| text_width(Font::Regular, 10.0, l, 0.0) <= 25.0));
        let e = ellipsize(
            Font::Bold,
            11.0,
            "Restaurante e Choperia do Zé da Esquina Ltda",
            40.0,
        );
        assert!(e.ends_with('…'));
        assert!(text_width(Font::Bold, 11.0, &e, 0.0) <= 40.0);
    }
}
