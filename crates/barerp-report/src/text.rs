//! Texto: codificação WinAnsi (cp1252) das fontes padrão e comparação como o `localeCompare` pt-BR.

use std::cmp::Ordering;

/// Byte WinAnsi do caractere; o que a tabela não tiver vira `?`.
pub(crate) fn winansi_byte(c: char) -> u8 {
    match c as u32 {
        0x20..=0x7E | 0xA0..=0xFF => c as u8,
        _ => match c {
            '€' => 0x80,
            '‚' => 0x82,
            'ƒ' => 0x83,
            '„' => 0x84,
            '…' => 0x85,
            '†' => 0x86,
            '‡' => 0x87,
            'ˆ' => 0x88,
            '‰' => 0x89,
            'Š' => 0x8A,
            '‹' => 0x8B,
            'Œ' => 0x8C,
            'Ž' => 0x8E,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '˜' => 0x98,
            '™' => 0x99,
            'š' => 0x9A,
            '›' => 0x9B,
            'œ' => 0x9C,
            'ž' => 0x9E,
            'Ÿ' => 0x9F,
            '\t' | '\n' | '\r' => b' ',
            _ => b'?',
        },
    }
}

pub(crate) fn winansi(s: &str) -> Vec<u8> {
    s.chars().map(winansi_byte).collect()
}

/// Letra base sem acento e minúscula, para comparar com `sensitivity: 'base'`.
fn fold(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

/// Ordem alfabética ignorando maiúsculas e acentos (`localeCompare(b, 'pt-BR', { sensitivity: 'base' })`).
pub(crate) fn compare_base(a: &str, b: &str) -> Ordering {
    a.chars().map(fold).cmp(b.chars().map(fold))
}

/// Como o `localeCompare` padrão: igual na base desempata pela grafia, para a ordem não depender da entrada.
pub(crate) fn compare_locale(a: &str, b: &str) -> Ordering {
    compare_base(a, b).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winansi_cobre_portugues() {
        assert_eq!(
            winansi("Balanço — ã"),
            vec![b'B', b'a', b'l', b'a', b'n', 0xE7, b'o', b' ', 0x97, b' ', 0xE3]
        );
        assert_eq!(winansi("✓"), vec![b'?']);
    }

    #[test]
    fn ordem_sem_acento_e_caixa() {
        let mut v = vec!["Água", "abacaxi", "Cerveja", "café", "Ácido"];
        v.sort_by(|a, b| compare_base(a, b));
        assert_eq!(v, vec!["abacaxi", "Ácido", "Água", "café", "Cerveja"]);
    }
}
