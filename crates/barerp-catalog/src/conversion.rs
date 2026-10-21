//! Sugestão do fator de conversão ao abrir a edição do produto (card "Fator de Conversão & Fracionamento"
//! do `ProductsView.tsx`): caixa cadastrada, maço de cigarro com 10, caixa de 12/24/6, ou peso em kg (porções).

use std::sync::LazyLock;

use regex::Regex;

static CIGARETTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)(?:10x20|box\s*10|cigarro|marlboro|malboro|rothmans|derby|hollywood|lucky\s*strike|camel|dunhill|winston|chesterfield|kent|parliament|san\s*marino|plaza|calton)(?-u:\b)").unwrap()
});
static TEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)10(?-u:\b)").unwrap());
static BOX12: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)(?:cx|c/|caixa)\s*12(?-u:\b)|(?-u:\b)12\s*(?:un|latas|lts|garrafas|gfs)(?-u:\b)").unwrap()
});
static BOX24: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)(?:cx|c/|fd|fardo|caixa)\s*24(?-u:\b)|(?-u:\b)24\s*(?:un|latas|lts|garrafas|gfs)(?-u:\b)").unwrap()
});
static PACK6: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)(?:pack|pct|cx|c/)\s*6(?-u:\b)|(?-u:\b)6\s*(?:un|latas|lts|garrafas|gfs)(?-u:\b)").unwrap()
});
static KG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)(\d+(?:[.,]\d+)?)\s*(?:kg|quilo|quilos)(?-u:\b)").unwrap());

/// (fator, porções?) sugerido para o produto.
pub fn suggest_conversion(name: &str, unit: &str, has_box: bool, box_quantity: Option<i64>) -> (u32, bool) {
    let n = name.to_lowercase();
    if has_box {
        if let Some(q) = box_quantity.filter(|q| *q > 1) {
            return (q as u32, false);
        }
    }
    if CIGARETTE.is_match(&n) && TEN.is_match(&n) {
        return (10, false);
    }
    if BOX12.is_match(&n) {
        return (12, false);
    }
    if BOX24.is_match(&n) {
        return (24, false);
    }
    if PACK6.is_match(&n) {
        return (6, false);
    }
    if KG.is_match(&n) || unit.eq_ignore_ascii_case("kg") {
        let w = KG.captures(&n).and_then(|c| c[1].replace(',', ".").parse::<f64>().ok()).unwrap_or(1.0);
        return (if w >= 4.0 { 12 } else if w >= 2.0 { 6 } else { 4 }, true);
    }
    (12, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sugestoes() {
        assert_eq!(suggest_conversion("Skol", "un", true, Some(15)), (15, false));
        assert_eq!(suggest_conversion("Cigarro Rothmans Box 10", "un", false, None), (10, false));
        assert_eq!(suggest_conversion("Rothmans azul", "un", false, None), (12, false), "sem o 10 não é maço");
        assert_eq!(suggest_conversion("Coca lata cx 12", "un", false, None), (12, false));
        assert_eq!(suggest_conversion("Brahma fardo 24", "un", false, None), (24, false));
        assert_eq!(suggest_conversion("Heineken pack 6", "un", false, None), (6, false));
        assert_eq!(suggest_conversion("Batata 5kg", "un", false, None), (12, true));
        assert_eq!(suggest_conversion("Batata 2,5 kg", "un", false, None), (6, true));
        assert_eq!(suggest_conversion("Picanha", "kg", false, None), (4, true));
        assert_eq!(suggest_conversion("Guaraná", "un", false, None), (12, false));
    }
}
