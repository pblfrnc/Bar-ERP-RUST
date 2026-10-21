//! DANFE NFC-e (cupom do consumidor) em ESC/POS, montado a partir do XML autorizado. Substitui o HTML da
//! Focus que o Electron mandava para a impressora (`printNfceDanfe`).

use barerp_fiscal::DanfeNfce;
use barerp_print::format::{cnpj, money};
use barerp_print::{Align, EscPos, QrEcc};

/// "2026-10-18T14:05:09-03:00" → "18/10/2026 14:05:09" (hora de Brasília, como veio na nota).
fn data_hora(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|d| d.format("%d/%m/%Y %H:%M:%S").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

fn qtd(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

fn documento(d: &str) -> String {
    if d.len() == 11 {
        format!("{}.{}.{}-{}", &d[0..3], &d[3..6], &d[6..9], &d[9..11])
    } else {
        cnpj(d)
    }
}

/// Escreve o cupom no documento (sem corte: quem imprime decide avanço e guilhotina).
pub fn desenhar(doc: &mut EscPos, d: &DanfeNfce, modulo_qr: u8) {
    // Emitente
    doc.align(Align::Center).bold(true);
    doc.line(d.emitente_fantasia.as_deref().unwrap_or(&d.emitente_nome));
    doc.bold(false);
    if d.emitente_fantasia.is_some() {
        doc.line(&d.emitente_nome);
    }
    let mut ident = format!("CNPJ: {}", cnpj(&d.emitente_cnpj));
    if let Some(ie) = &d.emitente_ie {
        ident.push_str(&format!("  IE: {ie}"));
    }
    doc.line(&ident);
    if !d.emitente_endereco.is_empty() {
        doc.line(&d.emitente_endereco);
    }
    doc.divider('-');
    doc.bold(true).line("DANFE NFC-e - Documento Auxiliar da Nota Fiscal de Consumidor Eletrônica").bold(false);
    if d.homologacao() {
        doc.divider('-');
        doc.bold(true).line("EMITIDA EM AMBIENTE DE HOMOLOGAÇÃO - SEM VALOR FISCAL").bold(false);
    }
    doc.divider('-');

    // Itens
    doc.align(Align::Left).bold(true).row("Cód. Descrição", "Vl. Total").bold(false);
    for i in &d.itens {
        doc.line(&format!("{} {}", i.codigo, i.descricao));
        doc.row(&format!("   {} {} x {}", qtd(i.quantidade), i.unidade, money(i.valor_unitario)), &money(i.valor_total));
        if i.desconto > 0.0 {
            doc.row("   Desconto", &format!("-{}", money(i.desconto)));
        }
    }
    doc.divider('-');

    // Totais e pagamento
    doc.row("Qtd. total de itens", &d.itens.len().to_string());
    doc.row("Valor total R$", &money(d.valor_produtos));
    if d.desconto > 0.0 {
        doc.row("Desconto R$", &money(d.desconto));
    }
    doc.bold(true).row("Valor a Pagar R$", &money(d.valor_total)).bold(false);
    doc.row("FORMA DE PAGAMENTO", "VALOR PAGO R$");
    for p in &d.pagamentos {
        doc.row(p.descricao(), &money(p.valor));
    }
    if d.troco > 0.0 {
        doc.row("Troco R$", &money(d.troco));
    }
    doc.divider('-');

    // Consulta
    doc.align(Align::Center).bold(true).line("Consulte pela Chave de Acesso em").bold(false);
    if !d.url_consulta.is_empty() {
        doc.line(&d.url_consulta);
    }
    doc.line(&d.chave_formatada());
    doc.divider('-');

    // Consumidor e identificação da nota
    match &d.consumidor_documento {
        Some(doc_cons) => {
            doc.bold(true).line(&format!("CONSUMIDOR - CPF/CNPJ: {}", documento(doc_cons))).bold(false);
            if let Some(n) = &d.consumidor_nome {
                doc.line(n);
            }
        }
        None => {
            doc.bold(true).line("CONSUMIDOR NÃO IDENTIFICADO").bold(false);
        }
    }
    doc.line(&format!(
        "NFC-e nº {:0>9} Série {:0>3} {}",
        d.numero,
        d.serie,
        data_hora(&d.data_emissao)
    ));
    if let Some(p) = &d.protocolo {
        doc.bold(true).line(&format!("Protocolo de autorização: {p}")).bold(false);
    }
    if let Some(dt) = &d.data_autorizacao {
        doc.line(&format!("Data de autorização: {}", data_hora(dt)));
    }
    if !d.qr_code.is_empty() {
        doc.feed(1);
        doc.qr(&d.qr_code, modulo_qr, QrEcc::M);
        doc.feed(1);
    }
    if let Some(t) = d.tributos {
        doc.line(&format!("Tributos totais incidentes (Lei Federal 12.741/2012): R$ {}", money(t)));
    }
    if let Some(info) = &d.info_complementar {
        doc.line(info);
    }
    doc.align(Align::Left);
}

#[cfg(test)]
mod tests {
    use super::*;
    use barerp_fiscal::{ItemDanfe, PagamentoDanfe};
    use barerp_print::{Codepage, PaperWidth};

    #[test]
    fn cupom_tem_os_campos_obrigatorios() {
        let d = DanfeNfce {
            chave: "15261036275163000124650010000000071794431341".into(),
            emitente_nome: "BAR DO ZE LTDA".into(),
            emitente_fantasia: Some("Bar do Zé".into()),
            emitente_cnpj: "36275163000124".into(),
            emitente_ie: Some("150000000".into()),
            emitente_endereco: "Av. Brasil, 100 - Umarizal - Belém/PA".into(),
            numero: "7".into(),
            serie: "1".into(),
            data_emissao: "2026-10-18T14:05:09-03:00".into(),
            ambiente: "2".into(),
            itens: vec![ItemDanfe {
                codigo: "5001".into(),
                descricao: "CERVEJA 600ML".into(),
                quantidade: 3.0,
                unidade: "UN".into(),
                valor_unitario: 12.5,
                valor_total: 37.5,
                desconto: 2.0,
            }],
            valor_produtos: 37.5,
            desconto: 2.0,
            valor_total: 35.5,
            tributos: Some(6.3),
            pagamentos: vec![PagamentoDanfe { codigo: "17".into(), valor: 35.5 }],
            troco: 0.0,
            consumidor_documento: None,
            consumidor_nome: None,
            qr_code: "https://sefa/qr?p=1".into(),
            url_consulta: "www.sefa.pa.gov.br/nfce/consulta".into(),
            protocolo: Some("315260000012345".into()),
            data_autorizacao: Some("2026-10-18T14:05:11-03:00".into()),
            info_complementar: None,
        };
        let mut doc = EscPos::new(PaperWidth::Mm80, Codepage::Pc850);
        desenhar(&mut doc, &d, 5);
        let t = doc.to_text();
        for esperado in [
            "Bar do Zé",
            // A quebra de linha da impressora junta os espaços e quebra a frase longa da homologação
            "CNPJ: 36.275.163/0001-24 IE: 150000000",
            "AMBIENTE DE HOMOLOGAÇÃO - SEM VALOR",
            "5001 CERVEJA 600ML",
            "3 UN x 12,50",
            "37,50",
            "Valor a Pagar R$",
            "35,50",
            "PIX",
            "1526 1036 2751",
            "CONSUMIDOR NÃO IDENTIFICADO",
            "NFC-e nº 000000007 Série 001 18/10/2026 14:05:09",
            "Protocolo de autorização: 315260000012345",
            "Tributos totais incidentes (Lei Federal",
            "12.741/2012): R$ 6,30",
        ] {
            assert!(t.contains(esperado), "faltou {esperado:?} em:\n{t}");
        }
        assert_eq!(qtd(1.5), "1,5");
        assert_eq!(documento("52998224725"), "529.982.247-25");
    }
}
