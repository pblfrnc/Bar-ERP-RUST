//! Chaves de ativação: offline compacta `EXP-AAAAMMDD-MAC8-SIG8` e legada (e-mail + HMAC permanente).
//! Mesmo algoritmo e mesmas mensagens do `settings.ts`.

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;

fn hmac_hex(secret: &str, data: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC aceita chave de qualquer tamanho");
    mac.update(data.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Chave legada esperada para o e-mail neste computador (`generateExpectedKey`).
pub fn legacy_key(secret: &str, email: &str, machine_id: &str) -> String {
    hmac_hex(secret, &format!("{email}:{machine_id}"))
}

/// Parte da máquina na chave offline: 8 primeiros caracteres alfanuméricos do ID, em maiúsculas.
pub fn machine_part(machine_id: &str) -> String {
    machine_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect::<String>().to_uppercase()
}

/// Data no formato do `toLocaleDateString('pt-BR')`.
pub fn br_date(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%d/%m/%Y").to_string()
}

/// Valida a chave offline e devolve o vencimento (23:59:59.999 UTC do dia da chave).
pub fn verify_offline_key(secret: &str, key: &str, machine_id: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let upper = key.trim().to_uppercase();
    let parts: Vec<&str> = upper.split('-').collect();
    if parts.len() != 4 || parts[0] != "EXP" {
        return Err("Formato de chave inválido. Formato esperado: EXP-AAAAMMDD-XXXX-YYYY".into());
    }
    let (exp, mac_part, sig_part) = (parts[1], parts[2], parts[3]);

    let clean_mac = machine_part(machine_id);
    if mac_part != clean_mac {
        return Err("Esta chave foi gerada para outro computador e não é válida nesta máquina.".into());
    }

    let expected_sig = hmac_hex(secret, &format!("{clean_mac}:{exp}"))[..8].to_uppercase();
    if sig_part != expected_sig {
        return Err("Código de validação incorreto ou chave adulterada.".into());
    }

    let expires_at = exp
        .get(0..8)
        .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y%m%d").ok())
        .and_then(|d| d.and_hms_milli_opt(23, 59, 59, 999))
        .map(|dt| Utc.from_utc_datetime(&dt))
        .ok_or_else(|| "Data de expiração inválida na chave.".to_string())?;

    if expires_at < now {
        return Err(format!("Esta chave de licença expirou em {}.", br_date(expires_at)));
    }
    Ok(expires_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "BAR_ERP_SUPER_SECRET_KEY_2026";
    // sha256("3f2504e0-4f89-11d3-9a0c-0305e82c3301"); chaves geradas com o código do Node
    const MID: &str = "d1bfaf4aff653cb27984b7d978e51a7d406d1572df95d205c254beb18dc134d3";

    #[test]
    fn offline_key_from_node() {
        let exp = verify_offline_key(SECRET, " exp-20991231-d1bfaf4a-6b75ca1e ", MID, Utc::now()).unwrap();
        assert_eq!(exp.to_rfc3339(), "2099-12-31T23:59:59.999+00:00");
    }

    #[test]
    fn offline_key_errors() {
        let now = Utc::now();
        assert!(verify_offline_key(SECRET, "ABC", MID, now).unwrap_err().starts_with("Formato de chave inválido"));
        assert!(verify_offline_key(SECRET, "EXP-20991231-AAAAAAAA-6B75CA1E", MID, now).unwrap_err().contains("outro computador"));
        assert!(verify_offline_key(SECRET, "EXP-20991231-D1BFAF4A-00000000", MID, now).unwrap_err().contains("adulterada"));
        let expired = verify_offline_key(SECRET, "EXP-20200101-D1BFAF4A-E574C4D3", MID, now).unwrap_err();
        assert!(expired.starts_with("Esta chave de licença expirou em "), "{expired}");
    }

    #[test]
    fn legacy_key_from_node() {
        assert_eq!(
            legacy_key(SECRET, "dono@bar.com", MID),
            "18e54c7afd34150b46deba1f27253f33fbe904be4b55dd636c971d47f49a1e6a"
        );
    }
}
