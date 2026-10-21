//! Builder ESC/POS: guarda os comandos e gera os bytes da impressora ou uma prévia em texto.

use crate::codepage::Codepage;

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;
const LF: u8 = 0x0A;

/// Largura da bobina; define as colunas da fonte A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaperWidth {
    Mm58,
    #[default]
    Mm80,
}

impl PaperWidth {
    pub fn columns(self) -> usize {
        match self {
            PaperWidth::Mm58 => 32,
            PaperWidth::Mm80 => 48,
        }
    }

    /// 58 ou 80, como salvo nas configurações da impressora; o resto vira 80mm.
    pub fn from_mm(mm: u32) -> Self {
        if mm == 58 {
            PaperWidth::Mm58
        } else {
            PaperWidth::Mm80
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Nível de correção de erro do QR Code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QrEcc {
    L,
    #[default]
    M,
    Q,
    H,
}

#[derive(Debug, Clone, PartialEq)]
enum Op {
    /// Linha já normalizada e cabendo nas colunas do momento.
    Line(String),
    Align(Align),
    Bold(bool),
    Invert(bool),
    Size {
        double_width: bool,
        double_height: bool,
    },
    Feed(u8),
    Qr {
        data: String,
        module: u8,
        ecc: QrEcc,
    },
    Cut {
        feed: u8,
    },
    /// Faixa de imagem 1 bit (bit 1 = ponto preto), `width_bytes` por linha.
    Raster {
        width_bytes: u16,
        height: u16,
        data: Vec<u8>,
    },
    Drawer(DrawerPin),
}

/// Pino do conector RJ-11 da gaveta (a maioria usa o 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawerPin {
    #[default]
    Pin2,
    Pin5,
}

/// Linhas por faixa de imagem: faixas curtas não estouram o buffer das impressoras mais simples.
const RASTER_BAND: usize = 128;

/// Documento ESC/POS. Os métodos encadeiam (`doc.bold(true).line("X").bold(false)`);
/// os textos são normalizados para a codepage e quebrados por palavra na largura atual
/// (metade das colunas com largura dupla ligada).
#[derive(Debug, Clone)]
pub struct EscPos {
    columns: usize,
    codepage: Codepage,
    codepage_number: u8,
    ops: Vec<Op>,
    align: Align,
    double_width: bool,
}

impl EscPos {
    pub fn new(paper: PaperWidth, codepage: Codepage) -> Self {
        Self::with_columns(paper.columns(), codepage)
    }

    /// Para impressoras com outra contagem de colunas (ex.: fonte B, 42 colunas).
    pub fn with_columns(columns: usize, codepage: Codepage) -> Self {
        Self {
            columns: columns.max(8),
            codepage,
            codepage_number: codepage.esc_t_number(),
            ops: Vec::new(),
            align: Align::Left,
            double_width: false,
        }
    }

    /// Troca o número do `ESC t n` para impressoras que não seguem a numeração Epson.
    pub fn codepage_number(&mut self, n: u8) -> &mut Self {
        self.codepage_number = n;
        self
    }

    pub fn codepage(&self) -> Codepage {
        self.codepage
    }

    /// Colunas disponíveis agora (metade com largura dupla).
    pub fn columns(&self) -> usize {
        if self.double_width {
            self.columns / 2
        } else {
            self.columns
        }
    }

    pub fn align(&mut self, align: Align) -> &mut Self {
        self.align = align;
        self.ops.push(Op::Align(align));
        self
    }

    pub fn bold(&mut self, on: bool) -> &mut Self {
        self.ops.push(Op::Bold(on));
        self
    }

    /// Texto branco em fundo preto.
    pub fn invert(&mut self, on: bool) -> &mut Self {
        self.ops.push(Op::Invert(on));
        self
    }

    pub fn size(&mut self, double_width: bool, double_height: bool) -> &mut Self {
        self.double_width = double_width;
        self.ops.push(Op::Size {
            double_width,
            double_height,
        });
        self
    }

    /// Largura e altura dupla juntas.
    pub fn double(&mut self, on: bool) -> &mut Self {
        self.size(on, on)
    }

    /// Texto com quebra por palavra; `\n` força nova linha. Texto vazio imprime uma linha em branco.
    pub fn line(&mut self, text: &str) -> &mut Self {
        self.wrapped("", "", text)
    }

    /// Texto quebrado com recuo: `first` antes da primeira linha e `rest` antes das demais.
    pub fn wrapped(&mut self, first: &str, rest: &str, text: &str) -> &mut Self {
        let width = self.columns();
        let first = self.codepage.normalize(first);
        let rest = self.codepage.normalize(rest);
        let mut prefix = first.as_str();
        for paragraph in text.split('\n') {
            let paragraph = self.codepage.normalize(paragraph);
            let avail = width.saturating_sub(char_len(prefix)).max(1);
            let mut lines = wrap(&paragraph, avail);
            if lines.is_empty() {
                lines.push(String::new());
            }
            for (i, l) in lines.into_iter().enumerate() {
                let p = if i == 0 { prefix } else { rest.as_str() };
                // O recuo das demais linhas pode ser diferente do primeiro: requebra se passar.
                if char_len(p) + char_len(&l) > width {
                    for part in wrap(&l, width.saturating_sub(char_len(p)).max(1)) {
                        self.ops.push(Op::Line(format!("{p}{part}")));
                    }
                } else {
                    self.ops
                        .push(Op::Line(format!("{p}{l}").trim_end().to_string()));
                }
                prefix = rest.as_str();
            }
        }
        self
    }

    /// `esquerda          direita` ocupando a linha toda.
    pub fn row(&mut self, left: &str, right: &str) -> &mut Self {
        self.row_with(left, right, ' ')
    }

    /// `esquerda ........ direita`.
    pub fn row_dotted(&mut self, left: &str, right: &str) -> &mut Self {
        self.row_with(left, right, '.')
    }

    /// Esquerda quebra se precisar e a direita fica na primeira linha, como o `flex justify-between` do recibo.
    pub fn row_with(&mut self, left: &str, right: &str, fill: char) -> &mut Self {
        let width = self.columns();
        let left = self.codepage.normalize(left);
        let right = self.codepage.normalize(right);
        let rlen = char_len(&right);
        if rlen + 2 > width {
            // Direita não cabe ao lado de nada: esquerda nas suas linhas, direita alinhada embaixo.
            for l in wrap(&left, width) {
                self.ops.push(Op::Line(l));
            }
            for r in wrap(&right, width) {
                self.ops.push(Op::Line(pad_left(&r, width)));
            }
            return self;
        }
        let lines = wrap(&left, width - rlen - 1);
        let mut lines = lines.into_iter();
        let first = lines.next().unwrap_or_default();
        let gap = width - char_len(&first) - rlen;
        let filler: String = if fill == ' ' || gap < 3 {
            " ".repeat(gap)
        } else {
            format!(" {} ", fill.to_string().repeat(gap - 2))
        };
        self.ops.push(Op::Line(format!("{first}{filler}{right}")));
        for l in lines {
            self.ops.push(Op::Line(l));
        }
        self
    }

    /// Linha inteira com o caractere (ex.: `-` ou `=`).
    pub fn divider(&mut self, ch: char) -> &mut Self {
        let ch = self
            .codepage
            .normalize(&ch.to_string())
            .chars()
            .next()
            .unwrap_or('-');
        let line = ch.to_string().repeat(self.columns());
        self.ops.push(Op::Line(line));
        self
    }

    /// Avança `lines` linhas em branco.
    pub fn feed(&mut self, lines: u8) -> &mut Self {
        if lines > 0 {
            self.ops.push(Op::Feed(lines));
        }
        self
    }

    /// QR Code nativo (`GS ( k`, modelo 2). `module` é o tamanho do ponto, de 1 a 16 (6 é um bom padrão em 80mm).
    pub fn qr(&mut self, data: &str, module: u8, ecc: QrEcc) -> &mut Self {
        self.ops.push(Op::Qr {
            data: data.to_string(),
            module: module.clamp(1, 16),
            ecc,
        });
        self
    }

    /// Avança até a guilhotina mais `feed` unidades e faz corte parcial (`GS V 66 n`).
    pub fn cut(&mut self, feed: u8) -> &mut Self {
        self.ops.push(Op::Cut { feed });
        self
    }

    /// Largura imprimível em pontos (12 pontos por coluna da fonte A: 576 em 80mm, 384 em 58mm).
    pub fn dots(&self) -> usize {
        self.columns * 12
    }

    /// Imagem em tons de cinza (`luma[y * width + x]`, 0 = preto, 255 = branco), impressa com
    /// `GS v 0`. Com `dither` usa Floyd–Steinberg (fotos); sem, corta em 50% (logos chapados).
    /// O que passar da largura do papel é cortado; o alinhamento atual centraliza a imagem.
    pub fn image(&mut self, width: usize, height: usize, luma: &[u8], dither: bool) -> &mut Self {
        if width == 0 || height == 0 || luma.len() < width * height {
            return self;
        }
        let out_w = width.min(self.dots());
        let bits = to_bits(width, height, luma, dither);
        let width_bytes = out_w.div_ceil(8);
        for band in (0..height).step_by(RASTER_BAND) {
            let rows = RASTER_BAND.min(height - band);
            let mut data = vec![0u8; width_bytes * rows];
            for y in 0..rows {
                for x in 0..out_w {
                    if bits[(band + y) * width + x] {
                        data[y * width_bytes + x / 8] |= 0x80 >> (x % 8);
                    }
                }
            }
            self.ops.push(Op::Raster {
                width_bytes: width_bytes as u16,
                height: rows as u16,
                data,
            });
        }
        self
    }

    /// Pulso para abrir a gaveta de dinheiro ligada na impressora (`ESC p`).
    pub fn open_drawer(&mut self, pin: DrawerPin) -> &mut Self {
        self.ops.push(Op::Drawer(pin));
        self
    }

    /// Bytes para mandar à impressora: começa com `ESC @` e a seleção da codepage.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![ESC, b'@', ESC, b't', self.codepage_number];
        for op in &self.ops {
            match op {
                Op::Line(s) => {
                    out.extend(self.codepage.encode(s));
                    out.push(LF);
                }
                Op::Align(a) => out.extend([ESC, b'a', *a as u8]),
                Op::Bold(on) => out.extend([ESC, b'E', *on as u8]),
                Op::Invert(on) => out.extend([GS, b'B', *on as u8]),
                Op::Size {
                    double_width,
                    double_height,
                } => {
                    let n = (if *double_width { 0x10 } else { 0 })
                        | (if *double_height { 0x01 } else { 0 });
                    out.extend([GS, b'!', n]);
                }
                Op::Feed(n) => out.extend([ESC, b'd', *n]),
                Op::Qr { data, module, ecc } => push_qr(&mut out, data.as_bytes(), *module, *ecc),
                Op::Cut { feed } => out.extend([GS, b'V', 66, *feed]),
                Op::Raster {
                    width_bytes,
                    height,
                    data,
                } => {
                    let [wl, wh] = width_bytes.to_le_bytes();
                    let [hl, hh] = height.to_le_bytes();
                    out.extend([GS, b'v', b'0', 0, wl, wh, hl, hh]);
                    out.extend_from_slice(data);
                }
                // Pulso de 50ms ligado e 500ms desligado
                Op::Drawer(pin) => out.extend([ESC, b'p', *pin as u8, 25, 250]),
            }
        }
        out
    }

    /// Prévia em texto puro com a largura do papel (para tela e testes). Largura dupla aparece
    /// com o alinhamento certo mas sem esticar as letras; QR e corte viram marcadores.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut align = Align::Left;
        let mut factor = 1;
        for op in &self.ops {
            match op {
                Op::Line(s) => {
                    let eff = self.columns / factor;
                    let free = eff.saturating_sub(char_len(s));
                    let pad = match align {
                        Align::Left => 0,
                        Align::Center => free / 2,
                        Align::Right => free,
                    };
                    out.push_str(&" ".repeat(pad * factor));
                    out.push_str(s);
                    out.push('\n');
                }
                Op::Align(a) => align = *a,
                Op::Size { double_width, .. } => factor = if *double_width { 2 } else { 1 },
                Op::Feed(n) => out.push_str(&"\n".repeat(*n as usize)),
                Op::Qr { data, .. } => {
                    out.push_str(&format!("[QR {data}]\n"));
                }
                Op::Cut { .. } => out.push_str(&format!("{}\n", "~".repeat(self.columns))),
                Op::Raster {
                    width_bytes,
                    height,
                    ..
                } => out.push_str(&format!(
                    "[IMAGEM {}x{}]\n",
                    *width_bytes as usize * 8,
                    height
                )),
                Op::Bold(_) | Op::Invert(_) | Op::Drawer(_) => {}
            }
        }
        out
    }
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn pad_left(s: &str, width: usize) -> String {
    format!("{}{s}", " ".repeat(width.saturating_sub(char_len(s))))
}

/// Quebra gulosa por palavra; palavra maior que a linha é cortada no meio.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let cur_len = char_len(&cur);
            let need = if cur.is_empty() {
                word.len()
            } else {
                cur_len + 1 + word.len()
            };
            if need <= width {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.extend(word.iter());
                break;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                continue;
            }
            let rest = word.split_off(width);
            lines.push(word.iter().collect());
            word = rest;
            if word.is_empty() {
                break;
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// `true` = ponto preto. Floyd–Steinberg espalha o erro de cada ponto para os vizinhos.
fn to_bits(width: usize, height: usize, luma: &[u8], dither: bool) -> Vec<bool> {
    let mut level: Vec<f32> = luma[..width * height].iter().map(|&v| v as f32).collect();
    let mut bits = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let black = level[i] < 128.0;
            bits[i] = black;
            if !dither {
                continue;
            }
            let err = level[i] - if black { 0.0 } else { 255.0 };
            let mut spread = |dx: isize, dy: usize, w: f32| {
                let nx = x as isize + dx;
                if nx >= 0 && (nx as usize) < width && y + dy < height {
                    level[(y + dy) * width + nx as usize] += err * w;
                }
            };
            spread(1, 0, 7.0 / 16.0);
            spread(-1, 1, 3.0 / 16.0);
            spread(0, 1, 5.0 / 16.0);
            spread(1, 1, 1.0 / 16.0);
        }
    }
    bits
}

fn push_qr(out: &mut Vec<u8>, data: &[u8], module: u8, ecc: QrEcc) {
    // Função 165: modelo 2
    out.extend([GS, b'(', b'k', 4, 0, 49, 65, 50, 0]);
    // Função 167: tamanho do módulo
    out.extend([GS, b'(', b'k', 3, 0, 49, 67, module]);
    // Função 169: correção de erro (48 = L … 51 = H)
    out.extend([GS, b'(', b'k', 3, 0, 49, 69, 48 + ecc as u8]);
    // Função 180: guarda os dados (limite do modelo 2: 7089 bytes)
    let data = &data[..data.len().min(7089)];
    let len = data.len() + 3;
    out.extend([
        GS,
        b'(',
        b'k',
        (len & 0xFF) as u8,
        (len >> 8) as u8,
        49,
        80,
        48,
    ]);
    out.extend_from_slice(data);
    // Função 181: imprime
    out.extend([GS, b'(', b'k', 3, 0, 49, 81, 48]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> EscPos {
        EscPos::new(PaperWidth::Mm58, Codepage::Cp860)
    }

    #[test]
    fn quebra_por_palavra() {
        assert_eq!(
            wrap("um dois tres quatro", 9),
            vec!["um dois", "tres", "quatro"]
        );
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert!(wrap("   ", 5).is_empty());
    }

    #[test]
    fn row_preenche_a_largura() {
        let mut d = doc();
        d.row("SUBTOTAL:", "R$ 10,00").row_dotted("Cerveja", "5,00");
        let text = d.to_text();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0], "SUBTOTAL:               R$ 10,00");
        assert_eq!(lines[1], "Cerveja ................... 5,00");
        assert!(lines.iter().all(|l| l.chars().count() == 32));
    }

    #[test]
    fn row_com_esquerda_longa_quebra_e_mantem_direita_na_primeira() {
        let mut d = doc();
        d.row("Cartão Crédito (Pessoa 1/2 da mesa grande)", "R$ 120,00");
        assert_eq!(
            d.to_text(),
            "Cartão Crédito (Pessoa R$ 120,00\n1/2 da mesa grande)\n"
        );
    }

    #[test]
    fn largura_dupla_usa_metade_das_colunas() {
        let mut d = doc();
        d.align(Align::Center)
            .double(true)
            .line("CONFERÊNCIA DE MESA");
        assert_eq!(d.columns(), 16);
        assert_eq!(d.to_text(), "  CONFERÊNCIA DE\n            MESA\n");
    }

    #[test]
    fn bytes_de_init_estilo_e_corte() {
        let mut d = doc();
        d.bold(true).line("Pão").bold(false).cut(3);
        assert_eq!(
            d.to_bytes(),
            vec![
                0x1B, b'@', 0x1B, b't', 3, 0x1B, b'E', 1, b'P', 0x84, b'o', 0x0A, 0x1B, b'E', 0,
                0x1D, b'V', 66, 3
            ]
        );
    }

    #[test]
    fn qr_nativo() {
        let mut d = doc();
        d.qr("ABC", 6, QrEcc::M);
        let bytes = d.to_bytes();
        assert_eq!(
            &bytes[5..],
            &[
                0x1D, b'(', b'k', 4, 0, 49, 65, 50, 0, //
                0x1D, b'(', b'k', 3, 0, 49, 67, 6, //
                0x1D, b'(', b'k', 3, 0, 49, 69, 49, //
                0x1D, b'(', b'k', 6, 0, 49, 80, 48, b'A', b'B', b'C', //
                0x1D, b'(', b'k', 3, 0, 49, 81, 48,
            ]
        );
    }

    #[test]
    fn gaveta_pino_2() {
        let mut d = doc();
        d.open_drawer(DrawerPin::Pin2);
        assert_eq!(&d.to_bytes()[5..], &[0x1B, b'p', 0, 25, 250]);
        assert_eq!(d.to_text(), "");
    }

    #[test]
    fn imagem_raster_1_bit() {
        // 10x2: primeira linha preta até x=8, segunda toda branca
        let mut luma = vec![255u8; 20];
        luma[..9].fill(0);
        let mut d = doc();
        d.image(10, 2, &luma, false);
        assert_eq!(
            &d.to_bytes()[5..],
            &[0x1D, b'v', b'0', 0, 2, 0, 2, 0, 0xFF, 0x80, 0x00, 0x00]
        );
        assert_eq!(d.to_text(), "[IMAGEM 16x2]\n");
    }

    #[test]
    fn imagem_alta_vira_faixas_e_larga_e_cortada() {
        let luma = vec![0u8; 500 * 300];
        let mut d = doc(); // 58mm = 384 pontos
        d.image(500, 300, &luma, true);
        let bytes = d.to_bytes();
        let headers: Vec<_> = bytes
            .windows(3)
            .enumerate()
            .filter(|(_, w)| w == &[0x1D, b'v', b'0'])
            .map(|(i, _)| {
                (
                    bytes[i + 4],
                    u16::from_le_bytes([bytes[i + 6], bytes[i + 7]]),
                )
            })
            .collect();
        assert_eq!(headers, vec![(48, 128), (48, 128), (48, 44)]);
    }

    #[test]
    fn dither_de_cinza_medio_da_metade_dos_pontos() {
        let luma = vec![128u8; 64 * 64];
        let bits = to_bits(64, 64, &luma, true);
        let pretos = bits.iter().filter(|b| **b).count();
        assert!((1900..=2200).contains(&pretos), "{pretos}");
        assert!(to_bits(64, 64, &luma, false).iter().all(|b| !b));
    }
}
