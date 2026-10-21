//! Criptografia de campos sensíveis e hash de senhas. Tradução de `server/src/services/securityVault.ts`.
//!
//! O formato é o mesmo do Node para abrir o `bar.db` atual sem migração:
//! - campos: `$enc$v1$<iv_hex>$<authTag_hex>$<ciphertext_hex>` (AES-256-GCM, chave scrypt)
//! - senhas: `$pbkdf2$<salt_hex>$<hash_hex>` (PBKDF2-SHA512, 10.000 iterações, salt usado como texto)

use std::sync::OnceLock;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use subtle::ConstantTimeEq;

const VAULT_SALT: &str = "BAR_ERP_VAULT_SALT_SECURE_2026_V1";
const DEFAULT_MASTER_SECRET: &str = "BAR_ERP_MILITARY_GRADE_VAULT_KEY_2026";
const ENC_PREFIX: &str = "$enc$v1$";
const PWD_PREFIX: &str = "$pbkdf2$";
const PBKDF2_ROUNDS: u32 = 10_000;
/// Valor devolvido quando o campo está cifrado mas não abre (chave errada ou dado adulterado).
pub const PROTECTED_PLACEHOLDER: &str = "***DADO_PROTEGIDO***";

fn encryption_key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(|| {
        let secret = std::env::var("DATABASE_VAULT_KEY").unwrap_or_else(|_| DEFAULT_MASTER_SECRET.to_string());
        // Mesmos parâmetros do crypto.scryptSync do Node: N=16384, r=8, p=1
        let params = scrypt::Params::new(14, 8, 1, 32).expect("parâmetros fixos do scrypt");
        let mut key = [0u8; 32];
        scrypt::scrypt(secret.as_bytes(), VAULT_SALT.as_bytes(), &params, &mut key).expect("tamanho fixo da chave");
        key
    })
}

/// Cifra o texto; texto vazio ou já cifrado volta como está.
pub fn encrypt_field(plain: &str) -> String {
    if plain.is_empty() || plain.starts_with(ENC_PREFIX) {
        return plain.to_string();
    }
    let cipher = Aes256Gcm::new(encryption_key().into());
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut iv);
    let sealed = cipher
        .encrypt(&Nonce::from(iv), plain.as_bytes())
        .expect("AES-GCM não falha com chave e IV de tamanho fixo");
    // O aes-gcm devolve cifra || tag; o formato do Node guarda a tag antes
    let (ciphertext, tag) = sealed.split_at(sealed.len() - 16);
    format!("{ENC_PREFIX}{}${}${}", hex::encode(iv), hex::encode(tag), hex::encode(ciphertext))
}

/// Abre um campo cifrado. Dado legado (não cifrado) volta como está.
pub fn decrypt_field(stored: &str) -> String {
    if !stored.starts_with(ENC_PREFIX) {
        return stored.to_string();
    }
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 6 {
        return stored.to_string();
    }
    let opened = (|| {
        let iv: [u8; 12] = hex::decode(parts[3]).ok()?.try_into().ok()?;
        let mut sealed = hex::decode(parts[5]).ok()?;
        sealed.extend(hex::decode(parts[4]).ok()?);
        let cipher = Aes256Gcm::new(encryption_key().into());
        let plain = cipher.decrypt(&Nonce::from(iv), sealed.as_slice()).ok()?;
        String::from_utf8(plain).ok()
    })();
    opened.unwrap_or_else(|| PROTECTED_PLACEHOLDER.to_string())
}

/// `Option` de conveniência para colunas anuláveis.
pub fn decrypt_opt(stored: Option<String>) -> Option<String> {
    stored.map(|s| decrypt_field(&s))
}

pub fn encrypt_opt(plain: Option<&str>) -> Option<String> {
    plain.map(encrypt_field)
}

fn pbkdf2_hex(password: &str, salt: &str) -> String {
    let mut out = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password.as_bytes(), salt.as_bytes(), PBKDF2_ROUNDS, &mut out);
    hex::encode(out)
}

/// Hash de senha/PIN de funcionário; senha já em hash volta como está.
pub fn hash_password(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    if plain.starts_with(PWD_PREFIX) {
        return plain.to_string();
    }
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let salt = hex::encode(salt);
    let hash = pbkdf2_hex(plain, &salt);
    format!("{PWD_PREFIX}{salt}${hash}")
}

/// Confere a senha; aceita senha legada em texto puro (migração transparente, como no Node).
pub fn verify_password(plain: &str, stored: &str) -> bool {
    if stored.is_empty() {
        return false;
    }
    if let Some(rest) = stored.strip_prefix(PWD_PREFIX) {
        let Some((salt, hash)) = rest.split_once('$') else { return false };
        if hash.contains('$') {
            return false;
        }
        return pbkdf2_hex(plain, salt).as_bytes().ct_eq(hash.as_bytes()).into();
    }
    stored == plain
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vetores gerados com o crypto do Node (mesmo código de securityVault.ts)
    const NODE_KEY: &str = "38b114abe07d9397fc78bd72c4252f0934f39fa3e760a159892eaa981dfe2ea4";
    const NODE_ENC: &str = "$enc$v1$070707070707070707070707$c42a5c1cc287a9150c681f6eec4d39f8$ba0fcc921474cdad52c20000cf8c3bda2757e8d4";
    const NODE_PWD: &str = "$pbkdf2$00112233445566778899aabbccddeeff$7c51383eea6e5ff0d0d8a80143212879dc2686b3160f2b297d8a967a6e04041d1b2015891c513b0ca58acf66b3db1d9b42458a928fe6305085cc8cb02d56d953";

    #[test]
    fn key_matches_node() {
        assert_eq!(hex::encode(encryption_key()), NODE_KEY);
    }

    #[test]
    fn opens_field_encrypted_by_node() {
        assert_eq!(decrypt_field(NODE_ENC), "token-focus-ÁÉ-123");
    }

    #[test]
    fn roundtrip_and_legacy() {
        let enc = encrypt_field("csc-secreto");
        assert!(enc.starts_with(ENC_PREFIX));
        assert_eq!(encrypt_field(&enc), enc);
        assert_eq!(decrypt_field(&enc), "csc-secreto");
        assert_eq!(decrypt_field("texto antigo"), "texto antigo");
        let tampered = format!("{}00", &enc[..enc.len() - 2]);
        assert_eq!(decrypt_field(&tampered), PROTECTED_PLACEHOLDER);
    }

    #[test]
    fn passwords() {
        assert!(verify_password("1234", NODE_PWD));
        assert!(!verify_password("4321", NODE_PWD));
        let h = hash_password("9876");
        assert!(verify_password("9876", &h));
        assert_eq!(hash_password(&h), h);
        assert!(verify_password("1234", "1234"));
        assert!(!verify_password("1234", ""));
    }
}
