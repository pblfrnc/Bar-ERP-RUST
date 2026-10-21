//! Sugestão de fator de conversão na entrada da nota (`detectConversionSuggestion` do FiscalImportView):
//! caixa/pacote pelo nome do item (CX12, FD 24, 10 maços de cigarro...) ou rendimento em porções quando o
//! item vem em quilos.

use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModoConversao {
    /// Caixa/embalagem: estoque em unidades.
    Caixa,
    /// Item em peso: estoque em porções.
    Porcao,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SugestaoConversao {
    pub fator: u32,
    pub modo: ModoConversao,
    pub detectada: bool,
}

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).expect("regex fixa"))
}

/// `\b` como no JavaScript (só ASCII).
macro_rules! rx {
    ($nome:ident, $p:expr) => {{
        static $nome: OnceLock<Regex> = OnceLock::new();
        re(&$nome, $p)
    }};
}

pub fn detectar_conversao(nome: &str, unidade: &str) -> SugestaoConversao {
    let t = nome.to_lowercase();
    let u = unidade.to_lowercase();
    let caixa = |fator| SugestaoConversao { fator, modo: ModoConversao::Caixa, detectada: true };

    // 1. Cigarros (10 maços por pacote/box)
    let pacote_cigarro = rx!(CIG1, r"(?i)(?-u:\b)(?:10x20|box\s*10|10\s*mac|10\s*pct|10\s*cart)(?-u:\b)");
    let marca_cigarro = rx!(
        CIG2,
        r"(?i)(?-u:\b)(?:cigarro|marlboro|malboro|rothmans|derby|hollywood|lucky\s*strike|camel|dunhill|winston|chesterfield|kent|parliament|san\s*marino|plaza|calton)(?-u:\b)"
    );
    let dez = rx!(DEZ, r"(?-u:\b)10(?-u:\b)");
    if pacote_cigarro.is_match(&t) || (marca_cigarro.is_match(&t) && dez.is_match(&t)) {
        return caixa(10);
    }
    // 2. Caixas com quantidade explícita
    if rx!(C12, r"(?i)(?-u:\b)(?:cx|c/|caixa)\s*12(?-u:\b)|(?-u:\b)12\s*(?:un|latas|lts|garrafas|gfs)(?-u:\b)").is_match(&t) {
        return caixa(12);
    }
    if rx!(C24, r"(?i)(?-u:\b)(?:cx|c/|fd|fardo|caixa)\s*24(?-u:\b)|(?-u:\b)24\s*(?:un|latas|lts|garrafas|gfs)(?-u:\b)").is_match(&t) {
        return caixa(24);
    }
    if rx!(C6, r"(?i)(?-u:\b)(?:pack|pct|cx|c/)\s*6(?-u:\b)|(?-u:\b)6\s*(?:un|latas|lts|garrafas|gfs)(?-u:\b)").is_match(&t) {
        return caixa(6);
    }
    if let Some(c) = rx!(CN, r"(?i)(?-u:\b)(?:cx|c/|caixa|fd|fardo|pack)\s*(\d{1,3})(?-u:\b)").captures(&t) {
        if let Ok(n) = c[1].parse::<u32>() {
            if n > 1 {
                return caixa(n);
            }
        }
    }
    // 3. Peso em kg: rendimento em porções (5 kg → 12 porções de ~400 g)
    let kg = rx!(KG, r"(?i)(?-u:\b)(\d+(?:[.,]\d+)?)\s*(?:kg|quilo|quilos)(?-u:\b)");
    let peso = match kg.captures(&t) {
        Some(c) => Some(c[1].replace(',', ".").parse::<f64>().unwrap_or(1.0)),
        None if u == "kg" => Some(1.0),
        None => None,
    };
    if let Some(p) = peso {
        let porcoes = if p >= 4.0 { 12 } else if p >= 2.0 { 6 } else { 4 };
        return SugestaoConversao { fator: porcoes, modo: ModoConversao::Porcao, detectada: true };
    }
    SugestaoConversao { fator: 1, modo: ModoConversao::Caixa, detectada: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sugestoes() {
        let f = |n: &str, u: &str| {
            let s = detectar_conversao(n, u);
            (s.fator, s.modo, s.detectada)
        };
        assert_eq!(f("MARLBORO BOX 10X20", "PCT"), (10, ModoConversao::Caixa, true));
        assert_eq!(f("CIGARRO DERBY 10 UN", "PC"), (10, ModoConversao::Caixa, true));
        assert_eq!(f("CERVEJA LATA 350ML CX12", "CX"), (12, ModoConversao::Caixa, true));
        assert_eq!(f("CERVEJA C/12 LATAS", "CX"), (12, ModoConversao::Caixa, true));
        assert_eq!(f("AGUA 24 GARRAFAS", "FD"), (24, ModoConversao::Caixa, true));
        assert_eq!(f("REFRI PACK 6", "PK"), (6, ModoConversao::Caixa, true));
        assert_eq!(f("SUCO CX 15", "CX"), (15, ModoConversao::Caixa, true));
        assert_eq!(f("BATATA PALITO 5KG", "PC"), (12, ModoConversao::Porcao, true));
        assert_eq!(f("QUEIJO 2,5 kg", "UN"), (6, ModoConversao::Porcao, true));
        assert_eq!(f("CALABRESA", "KG"), (4, ModoConversao::Porcao, true));
        assert_eq!(f("GELO", "UN"), (1, ModoConversao::Caixa, false));
    }
}
