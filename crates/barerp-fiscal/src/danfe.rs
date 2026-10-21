//! Ajuste do HTML do DANFE da NFC-e que a Focus devolve, para a bobina térmica (proxy `/api/fiscal/danfe`).
//! Tradução do trecho de HTML de `router.get('/danfe/:referencia')` em `fiscal.ts`.

use std::sync::OnceLock;

use regex::{Captures, Regex};

use crate::focus::url_seg;

/// Ajustes de impressão (tabela `PrinterSettings`) e nomes da empresa (`FiscalSettings`).
#[derive(Debug, Clone, PartialEq)]
pub struct AjustesDanfe {
    /// mm (padrão 80)
    pub largura_papel: f64,
    pub margem_topo: f64,
    pub margem_base: f64,
    pub margem_esquerda: f64,
    pub margem_direita: f64,
    /// px (padrão 100)
    pub tamanho_qr: f64,
    /// % (padrão 100)
    pub escala_fonte: f64,
    pub nome_fantasia: Option<String>,
    pub razao_social: Option<String>,
}

impl Default for AjustesDanfe {
    fn default() -> Self {
        Self {
            largura_papel: 80.0,
            margem_topo: 2.0,
            margem_base: 12.0,
            margem_esquerda: 1.0,
            margem_direita: 1.0,
            tamanho_qr: 100.0,
            escala_fonte: 100.0,
            nome_fantasia: None,
            razao_social: None,
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, padrao: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(padrao).expect("regex fixa"))
}

/// Nome fantasia em destaque no topo, QR Code da SEFAZ como `<img>` do tamanho configurado (sem o canvas
/// de 250 px da Focus) e CSS da bobina.
pub fn ajustar_danfe_nfce_html(html: &str, a: &AjustesDanfe) -> String {
    static EMPRESA: OnceLock<Regex> = OnceLock::new();
    static QR_TEXTO: OnceLock<Regex> = OnceLock::new();
    static QR_DIV: OnceLock<Regex> = OnceLock::new();
    static QR_SCRIPT: OnceLock<Regex> = OnceLock::new();
    static QR_MARGEM: OnceLock<Regex> = OnceLock::new();

    let mut html = html.to_string();
    let qr = a.tamanho_qr;

    // 0. Nome fantasia acima da razão social
    if let Some(fantasia) = a.nome_fantasia.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        let fantasia = fantasia.to_uppercase();
        let razao = a.razao_social.as_deref().unwrap_or("").trim().to_string();
        if !html.to_uppercase().contains(&fantasia) {
            let r = re(
                &EMPRESA,
                r#"(?i)<div class=['"]dados-da-empresa['"][^>]*>\s*<table[^>]*>\s*<tr>\s*<td>([^<]+)</td>"#,
            );
            html = r
                .replace(&html, |c: &Captures| {
                    let encontrada = c.get(1).map(|m| m.as_str()).filter(|s| !s.is_empty()).unwrap_or(&razao);
                    format!(
                        "<div class=\"dados-da-empresa\">\n      <table width=\"100%\" border=\"0\" cellpadding=\"2\" cellspacing=\"0\" style=\"font-size: 10px;\">\n        <tr>\n          <td style=\"font-size: 13px; font-weight: 900; text-align: center; text-transform: uppercase; padding-bottom: 2px;\">{fantasia}</td>\n        </tr>\n        <tr>\n          <td style=\"font-size: 9px; text-align: center; color: #444;\">{encontrada}</td>"
                    )
                })
                .into_owned();
        }
    }

    // 1. QR Code: URL da SEFAZ que está no script da Focus vira uma imagem
    let alvo = re(&QR_TEXTO, r#"text:\s*["']([^"']+)["']"#).captures(&html).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
    if let Some(alvo) = alvo {
        let img = format!(
            "<img src=\"https://api.qrserver.com/v1/create-qr-code/?size={qr}x{qr}&data={}&margin=1\" alt=\"QR Code NFC-e SEFAZ\" width=\"{qr}\" height=\"{qr}\" style=\"display:block;margin:0 auto;width:{qr}px;height:{qr}px;max-width:{qr}px;max-height:{qr}px;\" />",
            url_seg(&alvo)
        );
        let div = format!(
            "<div id=\"qr-code0\" style=\"margin:4px auto;text-align:center;display:flex;justify-content:center;align-items:center;width:{qr}px;min-height:{qr}px;\">{img}"
        );
        html = re(&QR_DIV, r#"(?i)<div id=['"]qr-code0['"][^>]*>"#).replace(&html, |_: &Captures| div.clone()).into_owned();
    }

    // Neutraliza o script da Focus (canvas de 250 px com margem de 36 px)
    html = re(&QR_SCRIPT, r"(?i)var\s+qrcode\s*=\s*new\s+QRCode\([\s\S]*?\);?")
        .replace_all(&html, "/* QRCode inline desativado pelo BarERP */")
        .into_owned();
    html = re(&QR_MARGEM, r#"(?i)document\.getElementById\(['"]qr-code\d*['"]\)\.style\.margin\s*=\s*["'][^"']+["'];?"#)
        .replace_all(&html, "/* Margem 36px neutralizada */")
        .into_owned();

    let estilo = estilo_bobina(a);
    match html.find("</head>") {
        Some(i) => html.insert_str(i, &estilo),
        None => html.insert_str(0, &estilo),
    }
    html
}

fn estilo_bobina(a: &AjustesDanfe) -> String {
    let (w, t, r, b, l, qr) =
        (a.largura_papel, a.margem_topo, a.margem_direita, a.margem_base, a.margem_esquerda, a.tamanho_qr);
    let zoom = a.escala_fonte / 100.0;
    format!(
        r#"
<style type="text/css">
  @page {{
    size: auto;
    margin: 0mm !important;
  }}
  @media print, all {{
    html, body {{
      width: 100% !important;
      max-width: 100% !important;
      margin: 0 !important;
      padding: 0 !important;
      background: #fff !important;
      box-sizing: border-box !important;
      zoom: {zoom} !important;
    }}
    .content {{
      max-width: {w}mm !important;
      width: 100% !important;
      margin: 0 !important;
      padding: {t}mm {r}mm {b}mm {l}mm !important;
      border: none !important;
      box-sizing: border-box !important;
    }}
    .tabela-nfce, table {{
      width: 100% !important;
      max-width: 100% !important;
      box-sizing: border-box !important;
    }}
    .qrcode {{
      width: 100% !important;
      margin: 4px 0 !important;
      text-align: center !important;
      display: block !important;
    }}
    #qr-code0, #qr-code1 {{
      margin: 4px auto !important;
      text-align: center !important;
      display: flex !important;
      justify-content: center !important;
      align-items: center !important;
      width: {qr}px !important;
      height: {qr}px !important;
      min-height: {qr}px !important;
      max-width: 100% !important;
      overflow: hidden !important;
    }}
    #qr-code0 img, #qr-code0 canvas, #qr-code0 svg, #qr-code1 img, #qr-code1 canvas, #qr-code1 svg {{
      width: {qr}px !important;
      height: {qr}px !important;
      max-width: {qr}px !important;
      max-height: {qr}px !important;
      margin: 0 auto !important;
      display: block !important;
    }}
  }}
</style>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"<html><head><title>NFC-e</title></head><body>
<div class="dados-da-empresa" id="x"><table border="0">
  <tr>
    <td>BAR DO ZE LTDA</td></tr></table></div>
<div id='qr-code0' class="q"></div>
<script>var qrcode = new QRCode(document.getElementById("qr-code0"), {
  text: "https://www.sefa.pa.gov.br/nfce?p=1526|2|1",
  width: 250 });
document.getElementById('qr-code0').style.margin = "36px";</script>
</body></html>"#;

    #[test]
    fn ajusta_cupom() {
        let a = AjustesDanfe {
            tamanho_qr: 120.0,
            escala_fonte: 90.0,
            nome_fantasia: Some(" Bar do Zé ".into()),
            ..Default::default()
        };
        let out = ajustar_danfe_nfce_html(HTML, &a);
        assert!(out.contains(">BAR DO ZÉ</td>"));
        assert!(out.contains("color: #444;\">BAR DO ZE LTDA</td>"));
        assert!(out.contains(
            "src=\"https://api.qrserver.com/v1/create-qr-code/?size=120x120&data=https%3A%2F%2Fwww.sefa.pa.gov.br%2Fnfce%3Fp%3D1526%7C2%7C1&margin=1\""
        ));
        assert!(out.contains("<div id=\"qr-code0\" style=\"margin:4px auto;"));
        assert!(out.contains("/* QRCode inline desativado pelo BarERP */"));
        assert!(!out.contains("new QRCode"));
        assert!(out.contains("/* Margem 36px neutralizada */"));
        assert!(out.contains("zoom: 0.9 !important;"));
        assert!(out.contains("max-width: 80mm !important;"));
        assert!(out.contains("padding: 2mm 1mm 12mm 1mm !important;"));
        assert!(out.find("<style type=\"text/css\">").unwrap() < out.find("</head>").unwrap());
    }

    #[test]
    fn sem_head_e_fantasia_ja_presente() {
        let html = "<div class='dados-da-empresa'><table><tr><td>BAR DO ZE</td></tr></table></div>";
        let a = AjustesDanfe { nome_fantasia: Some("bar do ze".into()), ..Default::default() };
        let out = ajustar_danfe_nfce_html(html, &a);
        assert!(out.starts_with("\n<style"));
        assert!(out.ends_with(html));
    }
}
