//! Números como o JavaScript do servidor Node os escreve e lê.

/// `Number.prototype.toFixed`: arredonda o valor exato do `f64`, empate para longe do zero.
/// (O `format!("{:.2}")` do Rust desempata para o par: 0.125 viraria "0.12" em vez de "0.13".)
pub fn to_fixed(v: f64, digits: usize) -> String {
    if !v.is_finite() {
        return if v.is_nan() { "NaN".into() } else if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    // 30 casas a mais mostram o valor exato do f64 até bem depois da casa decisiva
    let exact = format!("{:.*}", digits + 30, v.abs());
    let (int_part, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut num: Vec<u8> = int_part.bytes().chain(frac.bytes().take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|&d| d >= b'5') {
        let mut i = num.len();
        loop {
            if i == 0 {
                num.insert(0, b'1');
                break;
            }
            i -= 1;
            if num[i] == b'9' {
                num[i] = b'0';
            } else {
                num[i] += 1;
                break;
            }
        }
    }
    let split = num.len() - digits;
    let mut out = String::with_capacity(num.len() + 2);
    if v < 0.0 {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&num[..split]).unwrap_or("0"));
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&num[split..]).unwrap_or(""));
    }
    out
}

/// `Number(v.toFixed(2))`.
pub fn fixed2(v: f64) -> f64 {
    to_fixed(v, 2).parse().unwrap_or(0.0)
}

/// `parseFloat`: lê o maior prefixo numérico ("12.5abc" → 12.5). `None` se não houver número.
pub fn parse_float(s: &str) -> Option<f64> {
    let s = s.trim_start();
    let bytes = s.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < bytes.len() {
        match bytes[end] {
            b'0'..=b'9' => seen_digit = true,
            b'+' | b'-' if end == 0 || matches!(bytes[end - 1], b'e' | b'E') => {}
            b'.' if !seen_dot && !seen_exp => seen_dot = true,
            b'e' | b'E' if seen_digit && !seen_exp => seen_exp = true,
            _ => break,
        }
        end += 1;
    }
    // recua até um prefixo válido ("1e" → "1", "-" → nada)
    (1..=end).rev().find_map(|n| s[..n].parse::<f64>().ok())
}

/// Só os dígitos (`replace(/\D/g, '')`).
pub fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// `str.slice(0, n)` contando caracteres.
pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_como_js() {
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(1.005, 2), "1.00"); // 1.005 é 1.00499999... no f64
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(9.995, 2), "9.99");
        assert_eq!(to_fixed(9.999, 2), "10.00");
        assert_eq!(to_fixed(3.0, 4), "3.0000");
        assert_eq!(to_fixed(-0.125, 2), "-0.13");
        assert_eq!(to_fixed(0.1 + 0.2, 2), "0.30");
        assert_eq!(to_fixed(12345.678, 2), "12345.68");
    }

    #[test]
    fn parse_float_como_js() {
        assert_eq!(parse_float("12.5abc"), Some(12.5));
        assert_eq!(parse_float(" 7"), Some(7.0));
        assert_eq!(parse_float("1e3x"), Some(1000.0));
        assert_eq!(parse_float("1e"), Some(1.0));
        assert_eq!(parse_float("-"), None);
        assert_eq!(parse_float("abc"), None);
        assert_eq!(parse_float(""), None);
    }
}
