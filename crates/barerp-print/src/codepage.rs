//! Codepages das impressoras térmicas e transliteração do que não couber nelas.

use crate::codepage_tables::{CP860_HIGH, PC850_HIGH};

/// Tabela de caracteres selecionada com `ESC t n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codepage {
    /// Português (tem ã, õ, ç e maiúsculas acentuadas) — padrão.
    #[default]
    Cp860,
    /// Europa ocidental, para impressoras sem a CP860.
    Pc850,
}

impl Codepage {
    /// Número da tabela no `ESC t n` no padrão Epson (2 = PC850, 3 = PC860).
    pub fn esc_t_number(self) -> u8 {
        match self {
            Codepage::Cp860 => 3,
            Codepage::Pc850 => 2,
        }
    }

    fn high_table(self) -> &'static str {
        match self {
            Codepage::Cp860 => CP860_HIGH,
            Codepage::Pc850 => PC850_HIGH,
        }
    }

    /// Byte do caractere nesta codepage, se ela tiver.
    pub fn encode_char(self, c: char) -> Option<u8> {
        if c.is_ascii() {
            return Some(c as u8);
        }
        self.high_table()
            .chars()
            .position(|t| t == c)
            .map(|i| 0x80 + i as u8)
    }

    /// Troca o que a codepage não tem por equivalentes sem acento ou ASCII, e controles por espaço.
    /// Cada caractere do resultado vira exatamente um byte, então `chars().count()` é a largura impressa.
    pub fn normalize(self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if c == '\t' || c == '\n' || c == '\r' || c == '\u{a0}' {
                out.push(' ');
            } else if c.is_control() {
                continue;
            } else if self.encode_char(c).is_some() {
                out.push(c);
            } else {
                out.push_str(transliterate(c));
            }
        }
        out
    }

    /// Bytes de um texto já passado por [`Codepage::normalize`] (o que sobrar vira `?`).
    pub fn encode(self, s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| self.encode_char(c).unwrap_or(b'?'))
            .collect()
    }
}

/// Equivalente ASCII de um caractere que a codepage não tem.
fn transliterate(c: char) -> &'static str {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => "a",
        'Á' | 'À' | 'Â' | 'Ã' | 'Ä' | 'Å' => "A",
        'é' | 'è' | 'ê' | 'ë' => "e",
        'É' | 'È' | 'Ê' | 'Ë' => "E",
        'í' | 'ì' | 'î' | 'ï' => "i",
        'Í' | 'Ì' | 'Î' | 'Ï' => "I",
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => "o",
        'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => "O",
        'ú' | 'ù' | 'û' | 'ü' => "u",
        'Ú' | 'Ù' | 'Û' | 'Ü' => "U",
        'ç' => "c",
        'Ç' => "C",
        'ñ' => "n",
        'Ñ' => "N",
        'ª' => "a",
        'º' | '°' => "o",
        '•' | '·' | '∙' => "-",
        '–' | '—' | '−' => "-",
        '‘' | '’' | '´' | '`' => "'",
        '“' | '”' | '«' | '»' => "\"",
        '…' => "...",
        '€' => "EUR",
        '×' => "x",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp860_tem_acentos_do_portugues() {
        let cp = Codepage::Cp860;
        assert_eq!(cp.encode_char('Ç'), Some(0x80));
        assert_eq!(cp.encode_char('ã'), Some(0x84));
        assert_eq!(cp.encode_char('õ'), Some(0x94));
        assert_eq!(cp.encode_char('Ã'), Some(0x8E));
        assert_eq!(
            cp.normalize("Conferência de Mesa — Ação"),
            "Conferência de Mesa - Ação"
        );
    }

    #[test]
    fn pc850_cai_para_transliteracao_so_quando_precisa() {
        let cp = Codepage::Pc850;
        assert_eq!(cp.encode_char('ã'), Some(0xC6));
        assert_eq!(cp.encode_char('Õ'), Some(0xE5));
        assert_eq!(cp.normalize("SISTEMA • ✓ €"), "SISTEMA - ? EUR");
        assert_eq!(cp.encode("Pão"), vec![b'P', 0xC6, b'o']);
    }
}
