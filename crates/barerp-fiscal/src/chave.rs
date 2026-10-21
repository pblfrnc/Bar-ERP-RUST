//! Chave de acesso de 44 dígitos (`parseChaveAcessoNfe`) e distinção NFC-e / NF-e (`isNfce`).

use crate::num::digits;
use crate::payload::Modelo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChaveAcesso {
    /// Os 44 dígitos.
    pub chave: String,
    pub uf_cod: String,
    /// `AAAA-MM` da emissão.
    pub ano_mes: String,
    pub cnpj_emitente: String,
    /// `00.000.000/0000-00`.
    pub cnpj_formatado: String,
    /// "55" (NF-e) ou "65" (NFC-e).
    pub modelo: String,
    /// Sem zeros à esquerda.
    pub serie: String,
    /// Sem zeros à esquerda.
    pub numero: String,
    pub tp_emis: String,
}

/// Lê a chave (aceita pontuação e espaços). `None` se não tiver 44 dígitos.
pub fn parse_chave_acesso(chave: &str) -> Option<ChaveAcesso> {
    let c = digits(chave);
    if c.len() != 44 {
        return None;
    }
    let aamm = &c[2..6];
    let cnpj = &c[6..20];
    // parseInt(...).toString()
    let sem_zeros = |s: &str| match s.trim_start_matches('0') {
        "" => "0".to_string(),
        z => z.to_string(),
    };
    Some(ChaveAcesso {
        uf_cod: c[0..2].to_string(),
        ano_mes: format!("20{}-{}", &aamm[0..2], &aamm[2..4]),
        cnpj_emitente: cnpj.to_string(),
        cnpj_formatado: format!("{}.{}.{}/{}-{}", &cnpj[0..2], &cnpj[2..5], &cnpj[5..8], &cnpj[8..12], &cnpj[12..14]),
        modelo: c[20..22].to_string(),
        serie: sem_zeros(&c[22..25]),
        numero: sem_zeros(&c[25..34]),
        tp_emis: c[34..35].to_string(),
        chave: c,
    })
}

/// Modelo de uma nota emitida pela referência da Focus ou pela chave: NFC-e quando a referência começa com
/// `nfce_`/`cupom_` ou a chave é do modelo 65; todo o resto é NF-e.
pub fn modelo_da_nota(referencia: Option<&str>, chave: Option<&str>) -> Modelo {
    let por_ref = referencia.is_some_and(|r| r.starts_with("nfce_") || r.starts_with("cupom_"));
    let por_chave = chave.is_some_and(|c| c.len() == 44 && c.get(20..22) == Some("65"));
    if por_ref || por_chave {
        Modelo::Nfce
    } else {
        Modelo::Nfe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_chave() {
        let k = parse_chave_acesso("1526 0936 2751 6300 0124 5500 2000 0000 0117 9443 1341").unwrap();
        assert_eq!(k.chave, "15260936275163000124550020000000011794431341");
        assert_eq!(k.uf_cod, "15");
        assert_eq!(k.ano_mes, "2026-09");
        assert_eq!(k.cnpj_emitente, "36275163000124");
        assert_eq!(k.cnpj_formatado, "36.275.163/0001-24");
        assert_eq!(k.modelo, "55");
        assert_eq!(k.serie, "2");
        assert_eq!(k.numero, "1");
        assert_eq!(k.tp_emis, "1");
        assert!(parse_chave_acesso("123").is_none());
    }

    #[test]
    fn serie_zero() {
        let k = parse_chave_acesso("15260936275163000124650000000000011794431341").unwrap();
        assert_eq!(k.serie, "0");
        assert_eq!(k.modelo, "65");
    }

    #[test]
    fn modelo() {
        assert_eq!(modelo_da_nota(Some("nfce_abc"), None), Modelo::Nfce);
        assert_eq!(modelo_da_nota(Some("cupom_1"), None), Modelo::Nfce);
        assert_eq!(modelo_da_nota(Some("x"), Some("15260936275163000124650000000000011794431341")), Modelo::Nfce);
        assert_eq!(modelo_da_nota(Some("nfe_1"), None), Modelo::Nfe);
        assert_eq!(modelo_da_nota(None, None), Modelo::Nfe);
    }
}
