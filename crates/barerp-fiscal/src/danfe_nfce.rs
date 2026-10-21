//! Dados do DANFE NFC-e (cupom do consumidor) lidos do XML autorizado (`nfeProc`), para imprimir em
//! ESC/POS na bobina do caixa sem depender do HTML da Focus.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::FiscalError;
use crate::num::{digits, parse_float};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemDanfe {
    pub codigo: String,
    pub descricao: String,
    pub quantidade: f64,
    pub unidade: String,
    pub valor_unitario: f64,
    pub valor_total: f64,
    pub desconto: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PagamentoDanfe {
    /// Código tPag (01 dinheiro, 03 crédito, 04 débito, 17 PIX...).
    pub codigo: String,
    pub valor: f64,
}

impl PagamentoDanfe {
    /// Nome da forma de pagamento para o cupom.
    pub fn descricao(&self) -> &'static str {
        match self.codigo.as_str() {
            "01" => "Dinheiro",
            "02" => "Cheque",
            "03" => "Cartão de Crédito",
            "04" => "Cartão de Débito",
            "05" => "Crédito Loja",
            "10" => "Vale Alimentação",
            "11" => "Vale Refeição",
            "12" => "Vale Presente",
            "13" => "Vale Combustível",
            "15" => "Boleto Bancário",
            "16" => "Depósito Bancário",
            "17" => "PIX",
            "18" => "Transferência",
            "19" => "Fidelidade/Cashback",
            "90" => "Sem Pagamento",
            _ => "Outros",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DanfeNfce {
    pub chave: String,
    pub emitente_nome: String,
    pub emitente_fantasia: Option<String>,
    pub emitente_cnpj: String,
    pub emitente_ie: Option<String>,
    /// Logradouro, número, bairro, município/UF.
    pub emitente_endereco: String,
    pub numero: String,
    pub serie: String,
    pub data_emissao: String,
    /// 1 = produção, 2 = homologação.
    pub ambiente: String,
    pub itens: Vec<ItemDanfe>,
    pub valor_produtos: f64,
    pub desconto: f64,
    pub valor_total: f64,
    pub tributos: Option<f64>,
    pub pagamentos: Vec<PagamentoDanfe>,
    pub troco: f64,
    /// CPF ou CNPJ do consumidor (só dígitos), se identificado.
    pub consumidor_documento: Option<String>,
    pub consumidor_nome: Option<String>,
    pub qr_code: String,
    pub url_consulta: String,
    pub protocolo: Option<String>,
    pub data_autorizacao: Option<String>,
    pub info_complementar: Option<String>,
}

impl DanfeNfce {
    pub fn homologacao(&self) -> bool {
        self.ambiente == "2"
    }

    /// Chave em grupos de 4 dígitos, como no cupom.
    pub fn chave_formatada(&self) -> String {
        self.chave.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
    }
}

/// Lê o DANFE NFC-e de um XML autorizado (`nfeProc`, ou só a `NFe`).
pub fn ler_danfe_nfce(xml: &str) -> Result<DanfeNfce, FiscalError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let invalido = |e: quick_xml::Error| FiscalError::Validacao(format!("XML da NFC-e inválido: {e}"));
    let mut pilha: Vec<String> = Vec::new();
    let mut d = DanfeNfce::default();
    let mut ender: [Option<String>; 5] = Default::default();
    let mut item: Option<ItemDanfe> = None;
    let mut pag: Option<PagamentoDanfe> = None;
    let mut achou = false;
    loop {
        match reader.read_event().map_err(invalido)? {
            Event::Start(e) => {
                let nome = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if nome == "infNFe" {
                    achou = true;
                    if let Ok(Some(id)) = e.try_get_attribute("Id") {
                        d.chave = digits(&id.unescape_value().unwrap_or_default());
                    }
                }
                match nome.as_str() {
                    "det" => item = Some(ItemDanfe::default()),
                    "detPag" => pag = Some(PagamentoDanfe::default()),
                    _ => {}
                }
                pilha.push(nome);
            }
            Event::End(e) => {
                match e.local_name().as_ref() {
                    b"det" => d.itens.extend(item.take()),
                    b"detPag" => d.pagamentos.extend(pag.take()),
                    _ => {}
                }
                pilha.pop();
            }
            Event::Text(t) => {
                let v = t.unescape().map_err(invalido)?.into_owned();
                guardar(&mut d, &mut ender, item.as_mut(), pag.as_mut(), &pilha, v);
            }
            Event::CData(t) => {
                let v = String::from_utf8_lossy(&t).trim().to_string();
                guardar(&mut d, &mut ender, item.as_mut(), pag.as_mut(), &pilha, v);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !achou {
        return Err(FiscalError::Validacao("O arquivo não é o XML de uma NFC-e.".into()));
    }
    let [lgr, nro, bairro, mun, uf] = ender;
    let linha1 = [lgr, nro].into_iter().flatten().collect::<Vec<_>>().join(", ");
    let cidade = [mun, uf].into_iter().flatten().collect::<Vec<_>>().join("/");
    d.emitente_endereco =
        [Some(linha1), bairro, Some(cidade)].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" - ");
    Ok(d)
}

fn guardar(
    d: &mut DanfeNfce,
    ender: &mut [Option<String>; 5],
    item: Option<&mut ItemDanfe>,
    pag: Option<&mut PagamentoDanfe>,
    pilha: &[String],
    v: String,
) {
    if v.is_empty() {
        return;
    }
    let p: Vec<&str> = pilha.iter().map(String::as_str).collect();
    let fim = |sufixo: &[&str]| p.ends_with(sufixo);
    let num = |s: &str| parse_float(s).unwrap_or(0.0);
    if let Some(i) = item {
        if p.contains(&"prod") {
            match p.last().copied() {
                Some("cProd") => i.codigo = v,
                Some("xProd") => i.descricao = v,
                Some("qCom") => i.quantidade = num(&v),
                Some("uCom") => i.unidade = v,
                Some("vUnCom") => i.valor_unitario = num(&v),
                Some("vProd") => i.valor_total = num(&v),
                Some("vDesc") => i.desconto = num(&v),
                _ => {}
            }
        }
        return;
    }
    if let Some(pg) = pag {
        match p.last().copied() {
            Some("tPag") => pg.codigo = v,
            Some("vPag") => pg.valor = num(&v),
            _ => {}
        }
        return;
    }
    if fim(&["emit", "CNPJ"]) || fim(&["emit", "CPF"]) {
        d.emitente_cnpj = digits(&v);
    } else if fim(&["emit", "xNome"]) {
        d.emitente_nome = v;
    } else if fim(&["emit", "xFant"]) {
        d.emitente_fantasia = Some(v);
    } else if fim(&["emit", "IE"]) {
        d.emitente_ie = Some(v);
    } else if fim(&["enderEmit", "xLgr"]) {
        ender[0] = Some(v);
    } else if fim(&["enderEmit", "nro"]) {
        ender[1] = Some(v);
    } else if fim(&["enderEmit", "xBairro"]) {
        ender[2] = Some(v);
    } else if fim(&["enderEmit", "xMun"]) {
        ender[3] = Some(v);
    } else if fim(&["enderEmit", "UF"]) {
        ender[4] = Some(v);
    } else if fim(&["ide", "nNF"]) {
        d.numero = v;
    } else if fim(&["ide", "serie"]) {
        d.serie = v;
    } else if fim(&["ide", "dhEmi"]) {
        d.data_emissao = v;
    } else if fim(&["ide", "tpAmb"]) {
        d.ambiente = v;
    } else if fim(&["dest", "CPF"]) || fim(&["dest", "CNPJ"]) {
        d.consumidor_documento = Some(digits(&v));
    } else if fim(&["dest", "xNome"]) {
        d.consumidor_nome = Some(v);
    } else if fim(&["ICMSTot", "vProd"]) {
        d.valor_produtos = num(&v);
    } else if fim(&["ICMSTot", "vDesc"]) {
        d.desconto = num(&v);
    } else if fim(&["ICMSTot", "vNF"]) {
        d.valor_total = num(&v);
    } else if fim(&["ICMSTot", "vTotTrib"]) {
        d.tributos = Some(num(&v)).filter(|t| *t > 0.0);
    } else if fim(&["pag", "vTroco"]) {
        d.troco = num(&v);
    } else if fim(&["infNFeSupl", "qrCode"]) {
        d.qr_code = v;
    } else if fim(&["infNFeSupl", "urlChave"]) {
        d.url_consulta = v;
    } else if fim(&["infProt", "nProt"]) {
        d.protocolo = Some(v);
    } else if fim(&["infProt", "dhRecbto"]) {
        d.data_autorizacao = Some(v);
    } else if fim(&["infProt", "chNFe"]) && d.chave.is_empty() {
        d.chave = digits(&v);
    } else if fim(&["infAdic", "infCpl"]) {
        d.info_complementar = Some(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<nfeProc xmlns="http://www.portalfiscal.inf.br/nfe" versao="4.00"><NFe><infNFe Id="NFe15261036275163000124650010000000071794431341" versao="4.00">
<ide><serie>1</serie><nNF>7</nNF><dhEmi>2026-10-18T14:05:09-03:00</dhEmi><tpAmb>2</tpAmb></ide>
<emit><CNPJ>36275163000124</CNPJ><xNome>BAR DO ZE LTDA</xNome><xFant>Bar do Zé</xFant>
<enderEmit><xLgr>Av. Brasil</xLgr><nro>100</nro><xBairro>Umarizal</xBairro><xMun>Belém</xMun><UF>PA</UF></enderEmit><IE>150000000</IE></emit>
<dest><CPF>52998224725</CPF><xNome>Ana</xNome></dest>
<det nItem="1"><prod><cProd>5001</cProd><xProd>CERVEJA 600ML</xProd><uCom>UN</uCom><qCom>3.0000</qCom><vUnCom>12.50</vUnCom><vProd>37.50</vProd><vDesc>2.00</vDesc></prod></det>
<det nItem="2"><prod><cProd>2</cProd><xProd><![CDATA[PORÇÃO & CIA]]></xProd><uCom>UN</uCom><qCom>1</qCom><vUnCom>20.00</vUnCom><vProd>20.00</vProd></prod></det>
<total><ICMSTot><vProd>57.50</vProd><vDesc>2.00</vDesc><vNF>55.50</vNF><vTotTrib>9.80</vTotTrib></ICMSTot></total>
<pag><detPag><tPag>01</tPag><vPag>60.00</vPag></detPag><vTroco>4.50</vTroco></pag>
<infAdic><infCpl>Volte sempre</infCpl></infAdic></infNFe>
<infNFeSupl><qrCode><![CDATA[https://appnfc.sefa.pa.gov.br/portal/view/consultas/nfce/nfceForm.seam?p=1526|2|2|1|ABC]]></qrCode><urlChave>www.sefa.pa.gov.br/nfce/consulta</urlChave></infNFeSupl></NFe>
<protNFe versao="4.00"><infProt><tpAmb>2</tpAmb><chNFe>15261036275163000124650010000000071794431341</chNFe><dhRecbto>2026-10-18T14:05:11-03:00</dhRecbto><nProt>315260000012345</nProt></infProt></protNFe></nfeProc>"#;

    #[test]
    fn le_cupom() {
        let d = ler_danfe_nfce(XML).unwrap();
        assert_eq!(d.chave, "15261036275163000124650010000000071794431341");
        assert_eq!(d.chave_formatada().split(' ').count(), 11);
        assert_eq!((d.emitente_nome.as_str(), d.emitente_fantasia.as_deref()), ("BAR DO ZE LTDA", Some("Bar do Zé")));
        assert_eq!(d.emitente_endereco, "Av. Brasil, 100 - Umarizal - Belém/PA");
        assert_eq!((d.numero.as_str(), d.serie.as_str()), ("7", "1"));
        assert!(d.homologacao());
        assert_eq!(d.itens.len(), 2);
        assert_eq!(d.itens[0], ItemDanfe {
            codigo: "5001".into(), descricao: "CERVEJA 600ML".into(), quantidade: 3.0, unidade: "UN".into(),
            valor_unitario: 12.5, valor_total: 37.5, desconto: 2.0,
        });
        assert_eq!(d.itens[1].descricao, "PORÇÃO & CIA");
        assert_eq!((d.valor_produtos, d.desconto, d.valor_total, d.tributos), (57.5, 2.0, 55.5, Some(9.8)));
        assert_eq!(d.pagamentos, vec![PagamentoDanfe { codigo: "01".into(), valor: 60.0 }]);
        assert_eq!(d.pagamentos[0].descricao(), "Dinheiro");
        assert_eq!(d.troco, 4.5);
        assert_eq!((d.consumidor_documento.as_deref(), d.consumidor_nome.as_deref()), (Some("52998224725"), Some("Ana")));
        assert!(d.qr_code.ends_with("|ABC"));
        assert_eq!(d.url_consulta, "www.sefa.pa.gov.br/nfce/consulta");
        assert_eq!(d.protocolo.as_deref(), Some("315260000012345"));
        assert_eq!(d.info_complementar.as_deref(), Some("Volte sempre"));
        assert!(ler_danfe_nfce("<a/>").is_err());
    }
}
