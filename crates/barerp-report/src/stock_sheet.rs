//! Folha A4 de balanço de estoque e revisão de preços: impressa, preenchida à mão (contagem e
//! novo preço) e depois lançada no sistema. Tradução de `client/src/utils/stockSheet.ts`.

use chrono::NaiveDateTime;

use crate::pdf::{ascent, pt_mm, rgb, text_width, wrap, Anchor, Canvas, Font, Rgb, A4_H, A4_W};
use crate::text::{compare_base, compare_locale};

#[derive(Debug, Clone, PartialEq)]
pub struct StockCategory {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StockProduct {
    pub code: Option<String>,
    pub name: String,
    pub category_id: String,
    pub unit: String,
    pub stock: i64,
    pub track_stock: bool,
    pub price: f64,
    pub cost_price: Option<f64>,
    pub has_box_price: bool,
    pub box_quantity: Option<i64>,
    pub box_price: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SheetOrder {
    #[default]
    Alpha,
    Category,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StockSheetOptions {
    pub store_name: String,
    /// `None` = todas as categorias
    pub category_id: Option<String>,
    pub order: SheetOrder,
    pub show_cost: bool,
}

/// Bloco da folha: uma categoria (na ordem por categoria) ou a lista inteira (alfabética).
#[derive(Debug, Clone, PartialEq)]
pub struct StockGroup<'a> {
    pub category: Option<&'a StockCategory>,
    pub products: Vec<&'a StockProduct>,
}

/// Filtra e ordena os produtos como a folha mostra (`stockSheetProducts`).
pub fn stock_sheet_groups<'a>(
    products: &'a [StockProduct],
    categories: &'a [StockCategory],
    opts: &StockSheetOptions,
) -> Vec<StockGroup<'a>> {
    let by_name = |a: &&StockProduct, b: &&StockProduct| compare_base(&a.name, &b.name);
    let list: Vec<&StockProduct> = products
        .iter()
        .filter(|p| {
            opts.category_id
                .as_ref()
                .is_none_or(|c| &p.category_id == c)
        })
        .collect();
    if opts.order == SheetOrder::Alpha {
        let mut sorted = list;
        sorted.sort_by(by_name);
        return vec![StockGroup {
            category: None,
            products: sorted,
        }];
    }
    let mut cats: Vec<&StockCategory> = categories.iter().collect();
    cats.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| compare_locale(&a.name, &b.name))
    });
    cats.into_iter()
        .map(|c| {
            let mut ps: Vec<&StockProduct> = list
                .iter()
                .copied()
                .filter(|p| p.category_id == c.id)
                .collect();
            ps.sort_by(by_name);
            StockGroup {
                category: Some(c),
                products: ps,
            }
        })
        .filter(|g| !g.products.is_empty())
        .collect()
}

/// `R$ 1.234,56`; vazio quando não há valor.
fn brl(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_finite() => {
            let cents = (v.abs() * 100.0).round() as u64;
            let int = (cents / 100).to_string();
            let mut grouped = String::new();
            for (i, c) in int.chars().enumerate() {
                if i > 0 && (int.len() - i).is_multiple_of(3) {
                    grouped.push('.');
                }
                grouped.push(c);
            }
            let sign = if v < 0.0 && cents > 0 { "-" } else { "" };
            format!("{sign}R$ {grouped},{:02}", cents % 100)
        }
        _ => String::new(),
    }
}

/// Nome do arquivo sugerido ao salvar: `Balanço de estoque - Bebidas - 18-10-2026.pdf`.
pub fn stock_sheet_file_name(category_name: Option<&str>, generated_at: NaiveDateTime) -> String {
    let label = category_name.unwrap_or("todas as categorias");
    format!(
        "Balanço de estoque - {label} - {}.pdf",
        generated_at.format("%d-%m-%Y")
    )
}

const INK: Rgb = rgb(0x0c0a09);
const STONE_700: Rgb = rgb(0x44403c);
const STONE_600: Rgb = rgb(0x57534e);
const STONE_500: Rgb = rgb(0x78716c);
const STONE_400: Rgb = rgb(0xa8a29e);
const STONE_300: Rgb = rgb(0xd6d3d1);
const STONE_200: Rgb = rgb(0xe7e5e4);
const STONE_50: Rgb = rgb(0xfafaf9);
const AMBER_200: Rgb = rgb(0xfde68a);
const WHITE: Rgb = rgb(0xffffff);
/// Contagem do grupo com `opacity: 0.75` sobre o fundo preto
const GROUP_COUNT: Rgb = rgb(0xc2c2c1);

const MARGIN_TOP: f32 = 10.0;
const MARGIN_X: f32 = 9.0;
const MARGIN_BOTTOM: f32 = 12.0;
const PAD_X: f32 = 1.4;
const PAD_Y: f32 = 1.5;
const ROW_MIN_H: f32 = 7.5;
const BODY_PT: f32 = 8.5;
const SUB_PT: f32 = 7.0;
const HEAD_PT: f32 = 6.8;
const GROUP_PT: f32 = 9.0;

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Code,
    Name,
    Unit,
    Stock,
    Count,
    Cost,
    Price,
    NewPrice,
}

struct Column {
    kind: Col,
    title: &'static str,
    width: f32,
    anchor: Anchor,
    blank: bool,
}

fn columns(show_cost: bool) -> Vec<Column> {
    let c = |kind, title, width, anchor, blank| Column {
        kind,
        title,
        width,
        anchor,
        blank,
    };
    let mut cols = vec![
        c(Col::Code, "Cód.", 13.0, Anchor::Left, false),
        c(Col::Name, "Produto", 0.0, Anchor::Left, false),
        c(Col::Unit, "Un.", 9.0, Anchor::Center, false),
        c(Col::Stock, "Estoque", 14.0, Anchor::Right, false),
        c(Col::Count, "Contagem", 20.0, Anchor::Left, true),
    ];
    if show_cost {
        cols.push(c(Col::Cost, "Custo", 18.0, Anchor::Right, false));
    }
    cols.push(c(Col::Price, "Preço", 19.0, Anchor::Right, false));
    cols.push(c(Col::NewPrice, "Novo preço", 21.0, Anchor::Left, true));
    let fixed: f32 = cols.iter().map(|c| c.width).sum();
    let table_w = A4_W - 2.0 * MARGIN_X;
    cols[1].width = table_w - fixed;
    cols
}

/// Linha do produto já medida.
struct ProductRow<'a> {
    product: &'a StockProduct,
    name_lines: Vec<String>,
    sub_lines: Vec<String>,
    height: f32,
}

fn line_h(size: f32, factor: f32) -> f32 {
    pt_mm(size) * factor
}

fn measure<'a>(p: &'a StockProduct, sub: &str, name_w: f32) -> ProductRow<'a> {
    let inner = name_w - 2.0 * PAD_X;
    let name_lines = wrap(Font::Bold, BODY_PT, &p.name, inner);
    let sub_lines = if sub.is_empty() {
        Vec::new()
    } else {
        wrap(Font::Regular, SUB_PT, sub, inner)
    };
    let content = name_lines.len() as f32 * line_h(BODY_PT, 1.25)
        + sub_lines.len() as f32 * line_h(SUB_PT, 1.25);
    ProductRow {
        product: p,
        name_lines,
        sub_lines,
        height: (content + 2.0 * PAD_Y).max(ROW_MIN_H),
    }
}

struct Sheet {
    canvas: Canvas,
    cols: Vec<Column>,
    y: f32,
    /// Linhas do tbody na página, para o zebrado (`nth-child(even)`)
    body_index: usize,
}

impl Sheet {
    fn bottom() -> f32 {
        A4_H - MARGIN_BOTTOM
    }

    fn head_h() -> f32 {
        line_h(HEAD_PT, 1.2) + 2.0 * 1.6
    }

    fn group_h() -> f32 {
        line_h(GROUP_PT, 1.2) + 2.0 * 1.8
    }

    fn table_head(&mut self) {
        let h = Self::head_h();
        let mut x = MARGIN_X;
        for col in &self.cols {
            let bg = if col.blank { AMBER_200 } else { STONE_200 };
            self.canvas.fill_rect(x, self.y, col.width, h, bg);
            self.canvas
                .stroke_rect(x, self.y, col.width, h, STONE_400, 0.26, None);
            let tx = match col.anchor {
                Anchor::Left => x + PAD_X,
                Anchor::Center => x + col.width / 2.0,
                Anchor::Right => x + col.width - PAD_X,
            };
            let baseline = self.y + 1.6 + ascent(HEAD_PT);
            let title = col.title.to_uppercase();
            self.canvas.text(
                tx,
                baseline,
                col.anchor,
                Font::Bold,
                HEAD_PT,
                INK,
                &title,
                pt_mm(HEAD_PT) * 0.01,
            );
            x += col.width;
        }
        self.y += h;
        self.body_index = 0;
    }

    /// Quebra a página se `needed` não couber, repetindo o cabeçalho da tabela.
    fn ensure(&mut self, needed: f32) {
        if self.y + needed > Self::bottom() {
            self.canvas.new_page();
            self.y = MARGIN_TOP;
            self.table_head();
        }
    }

    fn group(&mut self, name: &str, count: usize) {
        let h = Self::group_h();
        let w = A4_W - 2.0 * MARGIN_X;
        self.body_index += 1;
        self.canvas.fill_rect(MARGIN_X, self.y, w, h, INK);
        let baseline = self.y + 1.8 + ascent(GROUP_PT);
        let x = MARGIN_X + 2.0;
        self.canvas.text(
            x,
            baseline,
            Anchor::Left,
            Font::Bold,
            GROUP_PT,
            WHITE,
            name,
            0.0,
        );
        let nx = x + text_width(Font::Bold, GROUP_PT, &format!("{name} "), 0.0);
        self.canvas.text(
            nx,
            baseline,
            Anchor::Left,
            Font::Regular,
            GROUP_PT,
            GROUP_COUNT,
            &format!("({count})"),
            0.0,
        );
        self.y += h;
    }

    fn product(&mut self, row: &ProductRow) {
        let p = row.product;
        self.body_index += 1;
        let zebra = self.body_index.is_multiple_of(2);
        let h = row.height;
        let mut x = MARGIN_X;
        let center_baseline = self.y + h / 2.0 + ascent(BODY_PT) / 2.0 - 0.15;
        for col in &self.cols {
            let bg = if col.blank {
                WHITE
            } else if zebra {
                STONE_50
            } else {
                WHITE
            };
            self.canvas.fill_rect(x, self.y, col.width, h, bg);
            let border = if col.blank { STONE_500 } else { STONE_300 };
            self.canvas
                .stroke_rect(x, self.y, col.width, h, border, 0.26, None);
            let right = x + col.width - PAD_X;
            match col.kind {
                Col::Code => {
                    let code = p.code.as_deref().unwrap_or("");
                    self.canvas.text(
                        x + PAD_X,
                        center_baseline,
                        Anchor::Left,
                        Font::Mono,
                        7.5,
                        STONE_600,
                        code,
                        0.0,
                    );
                }
                Col::Name => {
                    let block = row.name_lines.len() as f32 * line_h(BODY_PT, 1.25)
                        + row.sub_lines.len() as f32 * line_h(SUB_PT, 1.25);
                    let mut top = self.y + (h - block) / 2.0;
                    for l in &row.name_lines {
                        let b =
                            top + (line_h(BODY_PT, 1.25) - pt_mm(BODY_PT)) / 2.0 + ascent(BODY_PT);
                        self.canvas.text(
                            x + PAD_X,
                            b,
                            Anchor::Left,
                            Font::Bold,
                            BODY_PT,
                            INK,
                            l,
                            0.0,
                        );
                        top += line_h(BODY_PT, 1.25);
                    }
                    for l in &row.sub_lines {
                        let b = top + (line_h(SUB_PT, 1.25) - pt_mm(SUB_PT)) / 2.0 + ascent(SUB_PT);
                        self.canvas.text(
                            x + PAD_X,
                            b,
                            Anchor::Left,
                            Font::Regular,
                            SUB_PT,
                            STONE_500,
                            l,
                            0.0,
                        );
                        top += line_h(SUB_PT, 1.25);
                    }
                }
                Col::Unit => {
                    let unit = if p.unit.trim().is_empty() {
                        "un"
                    } else {
                        p.unit.as_str()
                    };
                    let cx = x + col.width / 2.0;
                    // "porção" não cabe nos 9mm: diminui a letra até caber (mínimo 5,5pt)
                    let mut size = BODY_PT;
                    while size > 5.5 && text_width(Font::Regular, size, unit, 0.0) > col.width - 1.0
                    {
                        size -= 0.25;
                    }
                    self.canvas.text(
                        cx,
                        center_baseline,
                        Anchor::Center,
                        Font::Regular,
                        size,
                        INK,
                        unit,
                        0.0,
                    );
                }
                Col::Stock => {
                    if p.track_stock {
                        let s = p.stock.to_string();
                        self.canvas.text(
                            right,
                            center_baseline,
                            Anchor::Right,
                            Font::Regular,
                            BODY_PT,
                            INK,
                            &s,
                            0.0,
                        );
                    } else {
                        self.canvas.text(
                            right,
                            center_baseline,
                            Anchor::Right,
                            Font::Regular,
                            BODY_PT,
                            STONE_400,
                            "—",
                            0.0,
                        );
                    }
                }
                Col::Cost => {
                    let s = brl(p.cost_price);
                    self.canvas.text(
                        right,
                        center_baseline,
                        Anchor::Right,
                        Font::Regular,
                        BODY_PT,
                        INK,
                        &s,
                        0.0,
                    );
                }
                Col::Price => {
                    let s = brl(Some(p.price));
                    self.canvas.text(
                        right,
                        center_baseline,
                        Anchor::Right,
                        Font::Bold,
                        BODY_PT,
                        INK,
                        &s,
                        0.0,
                    );
                }
                Col::Count | Col::NewPrice => {}
            }
            x += col.width;
        }
        self.y += h;
    }
}

/// PDF da folha de balanço. `generated_at` aparece no cabeçalho ("gerado em …").
pub fn stock_sheet_pdf(
    products: &[StockProduct],
    categories: &[StockCategory],
    opts: &StockSheetOptions,
    generated_at: NaiveDateTime,
) -> Vec<u8> {
    let groups = stock_sheet_groups(products, categories, opts);
    let total: usize = groups.iter().map(|g| g.products.len()).sum();
    let category_name = match &opts.category_id {
        None => "Todas".to_string(),
        Some(id) => categories
            .iter()
            .find(|c| &c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default(),
    };
    let show_category_label = opts.order == SheetOrder::Alpha && opts.category_id.is_none();

    let cols = columns(opts.show_cost);
    let name_w = cols[1].width;
    let mut sheet = Sheet {
        canvas: Canvas::new(),
        cols,
        y: MARGIN_TOP,
        body_index: 0,
    };
    let left = MARGIN_X;
    let right = A4_W - MARGIN_X;

    // Cabeçalho: loja + título à esquerda, filtros à direita, alinhados pela base
    let store_b = MARGIN_TOP + ascent(9.0);
    sheet.canvas.text(
        left,
        store_b,
        Anchor::Left,
        Font::Bold,
        9.0,
        STONE_600,
        &opts.store_name,
        0.0,
    );
    let title_b = store_b + line_h(9.0, 0.6) + line_h(14.0, 1.0);
    sheet.canvas.text(
        left,
        title_b,
        Anchor::Left,
        Font::Bold,
        14.0,
        INK,
        "Balanço de estoque e revisão de preços",
        0.0,
    );
    let meta_2 = title_b;
    let meta_1 = meta_2 - line_h(8.0, 1.5);
    let order = if opts.order == SheetOrder::Alpha {
        "alfabética"
    } else {
        "por categoria"
    };
    meta_line(
        &mut sheet.canvas,
        right,
        meta_1,
        &[
            ("Categoria: ", false),
            (&category_name, true),
            (" · Ordem: ", false),
            (order, true),
        ],
    );
    let produtos = if total == 1 { "produto" } else { "produtos" };
    let when = format!(
        " {produtos} · gerado em {} às {}",
        generated_at.format("%d/%m/%Y"),
        generated_at.format("%H:%M")
    );
    meta_line(
        &mut sheet.canvas,
        right,
        meta_2,
        &[(&total.to_string(), true), (&when, false)],
    );
    let rule_y = title_b + 3.0;
    sheet
        .canvas
        .fill_rect(left, rule_y, right - left, 0.53, INK);

    // Campos de assinatura
    let sign_y = rule_y + 0.53 + 3.0;
    let gap = 6.0;
    let sw = (right - left - 2.0 * gap) / 3.0;
    for (i, label) in ["Responsável:", "Data da contagem:", "Conferido por:"]
        .iter()
        .enumerate()
    {
        let x = left + i as f32 * (sw + gap);
        let b = sign_y + ascent(8.0);
        sheet.canvas.text(
            x,
            b,
            Anchor::Left,
            Font::Regular,
            8.0,
            STONE_700,
            label,
            0.0,
        );
        let ly = sign_y + line_h(8.0, 1.2) + 1.0;
        sheet.canvas.line(x, ly, x + sw, ly, STONE_400, 0.26);
    }
    sheet.y = sign_y + line_h(8.0, 1.2) + 1.0 + 3.0;
    sheet.table_head();

    for g in &groups {
        let rows: Vec<ProductRow> = g
            .products
            .iter()
            .map(|p| {
                let mut details = Vec::new();
                if show_category_label {
                    if let Some(c) = categories.iter().find(|c| c.id == p.category_id) {
                        details.push(c.name.clone());
                    }
                }
                if p.has_box_price {
                    if let Some(q) = p.box_quantity.filter(|q| *q > 0) {
                        let price = match p.box_price.filter(|v| *v != 0.0) {
                            Some(v) => format!(": {}", brl(Some(v))),
                            None => String::new(),
                        };
                        details.push(format!("Caixa c/ {q}{price}"));
                    }
                }
                measure(p, &details.join(" · "), name_w)
            })
            .collect();
        if let Some(c) = g.category {
            // Não deixa o título da categoria sozinho no pé da página
            let first = rows.first().map_or(0.0, |r| r.height);
            sheet.ensure(Sheet::group_h() + first);
            sheet.group(&c.name, g.products.len());
        }
        for r in &rows {
            sheet.ensure(r.height);
            sheet.product(r);
        }
    }

    let foot = "Preencha \"Contagem\" com a quantidade conferida e \"Novo preço\" só quando o preço mudar. \
                Estoque \"—\" = produto sem controle de estoque.";
    let lines = wrap(Font::Regular, 7.5, foot, right - left);
    let foot_h = 4.0 + lines.len() as f32 * line_h(7.5, 1.25);
    if sheet.y + foot_h > Sheet::bottom() {
        sheet.canvas.new_page();
        sheet.y = MARGIN_TOP;
    }
    let mut b = sheet.y + 4.0 + ascent(7.5);
    for l in &lines {
        sheet
            .canvas
            .text(left, b, Anchor::Left, Font::Regular, 7.5, STONE_500, l, 0.0);
        b += line_h(7.5, 1.25);
    }

    sheet.canvas.finish("Balanço de estoque")
}

/// Linha do canto direito com trechos normais e em negrito (`<b>`), alinhada à direita.
fn meta_line(canvas: &mut Canvas, right: f32, baseline: f32, parts: &[(&str, bool)]) {
    let font = |bold: bool| if bold { Font::Bold } else { Font::Regular };
    let total: f32 = parts
        .iter()
        .map(|(s, b)| text_width(font(*b), 8.0, s, 0.0))
        .sum();
    let mut x = right - total;
    for (s, bold) in parts {
        let color = if *bold { INK } else { STONE_700 };
        canvas.text(x, baseline, Anchor::Left, font(*bold), 8.0, color, s, 0.0);
        x += text_width(font(*bold), 8.0, s, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(id: &str, name: &str, order: i64) -> StockCategory {
        StockCategory {
            id: id.into(),
            name: name.into(),
            sort_order: order,
        }
    }

    fn prod(name: &str, cat: &str) -> StockProduct {
        StockProduct {
            code: None,
            name: name.into(),
            category_id: cat.into(),
            unit: "un".into(),
            stock: 10,
            track_stock: true,
            price: 10.0,
            cost_price: None,
            has_box_price: false,
            box_quantity: None,
            box_price: None,
        }
    }

    fn names<'a>(g: &StockGroup<'a>) -> Vec<&'a str> {
        g.products.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn alfabetica_ignora_acento_e_filtra_categoria() {
        let cats = vec![cat("b", "Bebidas", 0), cat("c", "Cozinha", 1)];
        let ps = vec![
            prod("Água", "b"),
            prod("Cerveja", "b"),
            prod("açaí", "c"),
            prod("Batata", "c"),
        ];
        let all = stock_sheet_groups(&ps, &cats, &StockSheetOptions::default());
        assert_eq!(all.len(), 1);
        assert_eq!(names(&all[0]), vec!["açaí", "Água", "Batata", "Cerveja"]);

        let only = StockSheetOptions {
            category_id: Some("b".into()),
            ..Default::default()
        };
        assert_eq!(
            names(&stock_sheet_groups(&ps, &cats, &only)[0]),
            vec!["Água", "Cerveja"]
        );
    }

    #[test]
    fn por_categoria_segue_sort_order_e_pula_vazias() {
        let cats = vec![
            cat("c", "Cozinha", 1),
            cat("x", "Vazia", 0),
            cat("b", "Bebidas", 1),
        ];
        let ps = vec![prod("Fritas", "c"), prod("Suco", "b"), prod("Água", "b")];
        let opts = StockSheetOptions {
            order: SheetOrder::Category,
            ..Default::default()
        };
        let g = stock_sheet_groups(&ps, &cats, &opts);
        let order: Vec<_> = g
            .iter()
            .map(|g| g.category.unwrap().name.as_str())
            .collect();
        assert_eq!(order, vec!["Bebidas", "Cozinha"]);
        assert_eq!(names(&g[0]), vec!["Água", "Suco"]);
    }

    #[test]
    fn dinheiro_e_nome_do_arquivo() {
        assert_eq!(brl(Some(1234.5)), "R$ 1.234,50");
        assert_eq!(brl(None), "");
        let at = NaiveDateTime::parse_from_str("2026-10-18 09:00", "%Y-%m-%d %H:%M").unwrap();
        assert_eq!(
            stock_sheet_file_name(None, at),
            "Balanço de estoque - todas as categorias - 18-10-2026.pdf"
        );
        assert_eq!(
            stock_sheet_file_name(Some("Bebidas"), at),
            "Balanço de estoque - Bebidas - 18-10-2026.pdf"
        );
    }
}
