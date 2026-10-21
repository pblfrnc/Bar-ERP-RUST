//! Formatação e validação fiscal (SEFAZ). Tradução de `server/src/services/fiscalDoc.ts`.

/// Valor a partir do qual a SEFAZ exige a identificação do consumidor (CPF/CNPJ) na NFC-e.
pub const NFCE_LIMITE_SEM_IDENTIFICACAO: f64 = 10_000.0;

fn digits(value: &str) -> Vec<u32> {
    value.chars().filter_map(|c| c.to_digit(10)).collect()
}

fn all_same(d: &[u32]) -> bool {
    d.windows(2).all(|w| w[0] == w[1])
}

/// Formata CPF (000.000.000-00) ou CNPJ (00.000.000/0000-00) conforme os dígitos são digitados.
pub fn format_cpf_cnpj(value: &str) -> String {
    let d: String = value.chars().filter(|c| c.is_ascii_digit()).take(14).collect();
    let mut out = String::with_capacity(18);
    let (dots, slash, dash): (&[usize], Option<usize>, usize) = if d.len() <= 11 {
        (&[3, 6], None, 9)
    } else {
        (&[2, 5], Some(8), 12)
    };
    for (i, c) in d.chars().enumerate() {
        if dots.contains(&i) {
            out.push('.');
        } else if slash == Some(i) {
            out.push('/');
        } else if i == dash {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// Dígitos verificadores do CPF (módulo 11).
pub fn validate_cpf(cpf: &str) -> bool {
    let d = digits(cpf);
    if d.len() != 11 || all_same(&d) {
        return false;
    }
    let check = |n: usize| {
        let sum: u32 = (0..n).map(|i| d[i] * (n as u32 + 1 - i as u32)).sum();
        let rev = 11 - (sum % 11);
        if rev >= 10 { 0 } else { rev }
    };
    check(9) == d[9] && check(10) == d[10]
}

/// Dígitos verificadores do CNPJ (módulo 11).
pub fn validate_cnpj(cnpj: &str) -> bool {
    let d = digits(cnpj);
    if d.len() != 14 || all_same(&d) {
        return false;
    }
    let check = |len: usize| {
        let mut pos = len as u32 - 7;
        let mut sum = 0;
        for &n in &d[..len] {
            sum += n * pos;
            pos -= 1;
            if pos < 2 {
                pos = 9;
            }
        }
        if sum % 11 < 2 { 0 } else { 11 - sum % 11 }
    };
    check(12) == d[12] && check(13) == d[13]
}

/// Mensagem de erro do documento, ou `None` se estiver vazio ou válido.
pub fn cpf_cnpj_error(value: &str) -> Option<&'static str> {
    let d = digits(value);
    match d.len() {
        0 => None,
        11 if validate_cpf(value) => None,
        11 => Some("CPF com dígitos verificadores inválidos."),
        14 if validate_cnpj(value) => None,
        14 => Some("CNPJ com dígitos verificadores inválidos."),
        _ => Some("Documento incompleto. Digite um CPF com 11 dígitos ou um CNPJ com 14 dígitos."),
    }
}

/// CEST com exatamente 7 dígitos (6 dígitos ganham o zero à esquerda). `None` se inválido.
pub fn sanitize_cest(value: &str) -> Option<String> {
    let raw: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    let padded = if raw.len() == 6 { format!("0{raw}") } else { raw };
    (padded.len() == 7 && padded != "0000000").then_some(padded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpf() {
        assert!(validate_cpf("529.982.247-25"));
        assert!(!validate_cpf("529.982.247-24"));
        assert!(!validate_cpf("111.111.111-11"));
        assert!(!validate_cpf("1234"));
    }

    #[test]
    fn cnpj() {
        assert!(validate_cnpj("36.275.163/0001-24"));
        assert!(validate_cnpj("68817608000147"));
        assert!(!validate_cnpj("36.275.163/0001-25"));
        assert!(!validate_cnpj("00000000000000"));
    }

    #[test]
    fn errors() {
        assert_eq!(cpf_cnpj_error(""), None);
        assert_eq!(cpf_cnpj_error("52998224725"), None);
        assert!(cpf_cnpj_error("5299822472").unwrap().starts_with("Documento incompleto"));
        assert!(cpf_cnpj_error("36275163000125").unwrap().starts_with("CNPJ"));
    }

    #[test]
    fn format() {
        assert_eq!(format_cpf_cnpj("52998224725"), "529.982.247-25");
        assert_eq!(format_cpf_cnpj("5299"), "529.9");
        assert_eq!(format_cpf_cnpj("36275163000124"), "36.275.163/0001-24");
        assert_eq!(format_cpf_cnpj("362751630"), "362.751.630");
    }

    #[test]
    fn cest() {
        assert_eq!(sanitize_cest("302103").as_deref(), Some("0302103"));
        assert_eq!(sanitize_cest("03.021.03").as_deref(), Some("0302103"));
        assert_eq!(sanitize_cest("0000000"), None);
        assert_eq!(sanitize_cest("123"), None);
    }
}
