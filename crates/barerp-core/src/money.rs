//! Valores em reais no formato brasileiro.

/// `1234.5` → `"R$ 1.234,50"`.
pub fn brl(value: f64) -> String {
    let cents = (value.abs() * 100.0).round() as u64;
    let (int, frac) = (cents / 100, cents % 100);
    let digits = int.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(c);
    }
    let sign = if value < 0.0 && cents > 0 { "-" } else { "" };
    format!("{sign}R$ {grouped},{frac:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(brl(0.0), "R$ 0,00");
        assert_eq!(brl(87.5), "R$ 87,50");
        assert_eq!(brl(1234.567), "R$ 1.234,57");
        assert_eq!(brl(1_000_000.0), "R$ 1.000.000,00");
        assert_eq!(brl(-12.3), "-R$ 12,30");
    }
}
