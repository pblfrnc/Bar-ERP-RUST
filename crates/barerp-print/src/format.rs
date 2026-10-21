//! Formatos brasileiros usados nos comprovantes (dinheiro, CNPJ, data e hora).

use chrono::NaiveDateTime;

/// `1.234,56` — duas casas, como `toLocaleString('pt-BR', { minimumFractionDigits: 2 })`.
pub fn money(v: f64) -> String {
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
    format!("{sign}{grouped},{:02}", cents % 100)
}

/// `12.345.678/0001-90` quando tem 14 dígitos; senão devolve como veio.
pub fn cnpj(value: &str) -> String {
    let d: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() != 14 {
        return value.to_string();
    }
    format!(
        "{}.{}.{}/{}-{}",
        &d[0..2],
        &d[2..5],
        &d[5..8],
        &d[8..12],
        &d[12..14]
    )
}

/// `18/10/2026 21:05`
pub fn date_time(d: NaiveDateTime) -> String {
    d.format("%d/%m/%Y %H:%M").to_string()
}

/// `21:05:09`
pub fn time_seconds(d: NaiveDateTime) -> String {
    d.format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dinheiro() {
        assert_eq!(money(0.0), "0,00");
        assert_eq!(money(5.5), "5,50");
        assert_eq!(money(1234.567), "1.234,57");
        assert_eq!(money(1_000_000.0), "1.000.000,00");
        assert_eq!(money(-12.3), "-12,30");
        assert_eq!(money(-0.001), "0,00");
    }

    #[test]
    fn cnpj_formatado() {
        assert_eq!(cnpj("12345678000190"), "12.345.678/0001-90");
        assert_eq!(cnpj("12.345.678/0001-90"), "12.345.678/0001-90");
        assert_eq!(cnpj("123"), "123");
    }
}
