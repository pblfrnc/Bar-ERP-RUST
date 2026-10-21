//! Leitura do XML de NF-e recebida (importação de compra e sincronização das notas da SEFAZ).
//!
//! Tradução das leituras com `fast-xml-parser` de `fiscal.ts` (`/import-xml`, `valorRecebidaComFallback`) e
//! de `nfeRecebidasScheduler.ts`. Diferença: os campos ficam como texto, então NCM e EAN com zero à
//! esquerda chegam inteiros (o `fast-xml-parser` os convertia em número e perdia o zero).

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::error::FiscalError;
use crate::num::{digits, parse_float};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmitenteXml {
    /// CNPJ (ou CPF) só com dígitos.
    pub documento: String,
    pub nome: String,
    pub fantasia: Option<String>,
    pub ie: Option<String>,
    pub fone: Option<String>,
    /// Logradouro, número e bairro separados por vírgula.
    pub endereco: Option<String>,
    pub municipio: Option<String>,
    pub uf: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemXml {
    pub codigo: Option<String>,
    /// `None` quando ausente ou "SEM GTIN".
    pub ean: Option<String>,
    pub nome: String,
    pub quantidade: f64,
    pub custo_unitario: f64,
    pub ncm: Option<String>,
    pub cfop: Option<String>,
    /// `uCom`; "un" quando ausente.
    pub unidade: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NfeXml {
    /// Os 44 dígitos do `Id` de `infNFe` (sem o prefixo "NFe"). Vazio se ausente.
    pub chave_acesso: String,
    pub emitente: EmitenteXml,
    pub numero: String,
    pub serie: String,
    /// `dhEmi` ou `dEmi`. `None` se a nota não tiver (o Node usa a hora atual).
    pub data_emissao: Option<String>,
    /// `vNF`; 0 se ausente.
    pub valor_total: f64,
    pub itens: Vec<ItemXml>,
}

/// O texto é um XML de NF-e completo (e não só o resumo)?
pub fn parece_xml_nfe(texto: &str) -> bool {
    texto.contains("<nfeProc") || texto.contains("<NFe") || texto.starts_with("<?xml")
}

#[derive(Default)]
struct Leitura {
    achou_inf: bool,
    chave: String,
    emit: EmitenteXml,
    ender: [Option<String>; 3],
    numero: String,
    serie: String,
    dh_emi: Option<String>,
    d_emi: Option<String>,
    vnf: Option<String>,
    res_vnf: Option<String>,
    itens: Vec<ItemXml>,
    item: Option<ItemXml>,
}

fn nome_local(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn ler(xml: &str) -> Result<Leitura, FiscalError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut pilha: Vec<String> = Vec::new();
    let mut l = Leitura::default();
    // posição de infNFe na pilha (só a primeira nota do arquivo)
    let mut inf: Option<usize> = None;
    let invalido = |e: quick_xml::Error| FiscalError::Validacao(format!("XML de NF-e inválido: {e}"));
    loop {
        match reader.read_event().map_err(invalido)? {
            Event::Start(e) => {
                let nome = nome_local(&e);
                if nome == "infNFe" && !l.achou_inf {
                    l.achou_inf = true;
                    inf = Some(pilha.len());
                    if let Ok(Some(id)) = e.try_get_attribute("Id") {
                        let id = id.unescape_value().map(|v| v.into_owned()).unwrap_or_default();
                        l.chave = id.replace("NFe", "").trim().to_string();
                    }
                }
                pilha.push(nome);
                if let Some(base) = inf {
                    if pilha.len() == base + 2 && pilha[base + 1] == "det" {
                        l.item = Some(ItemXml { unidade: "un".into(), ..Default::default() });
                    }
                }
            }
            Event::End(_) => {
                if let Some(base) = inf {
                    if pilha.len() == base + 2 && pilha[base + 1] == "det" {
                        l.itens.extend(l.item.take());
                    }
                    if pilha.len() == base + 1 {
                        inf = None; // fim do infNFe
                    }
                }
                pilha.pop();
            }
            Event::Text(t) => {
                let texto = t.unescape().map_err(invalido)?.into_owned();
                guardar(&mut l, &pilha, inf, texto);
            }
            Event::CData(t) => {
                let texto = String::from_utf8_lossy(&t).trim().to_string();
                guardar(&mut l, &pilha, inf, texto);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(l)
}

fn guardar(l: &mut Leitura, pilha: &[String], inf: Option<usize>, texto: String) {
    if texto.is_empty() {
        return;
    }
    let Some(base) = inf else {
        // resumo da nota (resNFe), que a SEFAZ libera antes do XML completo
        if pilha.len() == 2 && pilha[0] == "resNFe" && pilha[1] == "vNF" {
            l.res_vnf = Some(texto);
        }
        return;
    };
    let caminho: Vec<&str> = pilha[base + 1..].iter().map(String::as_str).collect();
    let e = &mut l.emit;
    match caminho.as_slice() {
        ["ide", "nNF"] => l.numero = texto,
        ["ide", "serie"] => l.serie = texto,
        ["ide", "dhEmi"] => l.dh_emi = Some(texto),
        ["ide", "dEmi"] => l.d_emi = Some(texto),
        ["emit", "CNPJ"] => e.documento = digits(&texto),
        ["emit", "CPF"] if e.documento.is_empty() => e.documento = digits(&texto),
        ["emit", "xNome"] => e.nome = texto,
        ["emit", "xFant"] => e.fantasia = Some(texto),
        ["emit", "IE"] => e.ie = Some(texto),
        ["emit", "enderEmit", "fone"] => e.fone = Some(texto),
        ["emit", "enderEmit", "xLgr"] => l.ender[0] = Some(texto),
        ["emit", "enderEmit", "nro"] => l.ender[1] = Some(texto),
        ["emit", "enderEmit", "xBairro"] => l.ender[2] = Some(texto),
        ["emit", "enderEmit", "xMun"] => e.municipio = Some(texto),
        ["emit", "enderEmit", "UF"] => e.uf = Some(texto),
        ["total", "ICMSTot", "vNF"] => l.vnf = Some(texto),
        ["det", "prod", campo] => {
            if let Some(item) = l.item.as_mut() {
                match *campo {
                    "cProd" => item.codigo = Some(texto),
                    "cEAN" if texto != "SEM GTIN" => item.ean = Some(texto),
                    "xProd" => item.nome = texto,
                    "qCom" => item.quantidade = parse_float(&texto).unwrap_or(0.0),
                    "vUnCom" => item.custo_unitario = parse_float(&texto).unwrap_or(0.0),
                    "NCM" => item.ncm = Some(texto),
                    "CFOP" => item.cfop = Some(texto),
                    "uCom" => item.unidade = texto,
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Lê um XML de NF-e (`nfeProc` autorizado ou só a `NFe`).
pub fn ler_nfe_xml(xml: &str) -> Result<NfeXml, FiscalError> {
    let l = ler(xml)?;
    if !l.achou_inf {
        return Err(FiscalError::validacao("O arquivo não parece ser um XML válido de NF-e."));
    }
    let mut emitente = l.emit;
    if emitente.nome.is_empty() {
        emitente.nome = "Fornecedor sem nome".into();
    }
    let endereco: Vec<String> = l.ender.into_iter().flatten().collect();
    emitente.endereco = (!endereco.is_empty()).then(|| endereco.join(", "));
    Ok(NfeXml {
        chave_acesso: l.chave,
        emitente,
        numero: l.numero,
        serie: l.serie,
        data_emissao: l.dh_emi.or(l.d_emi),
        valor_total: l.vnf.as_deref().and_then(parse_float).unwrap_or(0.0),
        itens: l.itens,
    })
}

/// `vNF` da nota completa ou, na falta dela, do resumo `resNFe`. `None` se não houver valor positivo.
/// Usado para corrigir notas recebidas gravadas com valor zerado (`valorRecebidaComFallback`).
pub fn valor_nf(xml: &str) -> Option<f64> {
    let l = ler(xml).ok()?;
    l.vnf.or(l.res_vnf).as_deref().and_then(parse_float).filter(|v| *v > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NFE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<nfeProc xmlns="http://www.portalfiscal.inf.br/nfe" versao="4.00">
  <NFe xmlns="http://www.portalfiscal.inf.br/nfe">
    <infNFe Id="NFe15260936275163000124550020000000011794431341" versao="4.00">
      <ide><cUF>15</cUF><serie>2</serie><nNF>1</nNF><dhEmi>2026-09-11T12:00:00-03:00</dhEmi></ide>
      <emit>
        <CNPJ>36275163000124</CNPJ>
        <xNome>Distribuidora &amp; Bebidas LTDA</xNome>
        <xFant>Distri</xFant>
        <enderEmit><xLgr>Rua A</xLgr><nro>10</nro><xBairro>Centro</xBairro><xMun>Belém</xMun><UF>PA</UF><fone>9132220000</fone></enderEmit>
        <IE>150000000</IE>
      </emit>
      <det nItem="1">
        <prod><cProd>001</cProd><cEAN>07891234567895</cEAN><xProd>CERVEJA LATA 350ML</xProd><NCM>02203000</NCM>
          <CFOP>5405</CFOP><uCom>CX</uCom><qCom>10.0000</qCom><vUnCom>45.9000000000</vUnCom></prod>
        <imposto><ICMS><ICMS60><vNF>999</vNF></ICMS60></ICMS></imposto>
      </det>
      <det nItem="2">
        <prod><cProd>002</cProd><cEAN>SEM GTIN</cEAN><xProd><![CDATA[GELO 5KG]]></xProd><NCM>22019000</NCM>
          <CFOP>5102</CFOP><qCom>2</qCom><vUnCom>8.5</vUnCom></prod>
      </det>
      <total><ICMSTot><vProd>476.00</vProd><vNF>476.00</vNF></ICMSTot></total>
    </infNFe>
    <Signature><SignedInfo/></Signature>
  </NFe>
  <protNFe versao="4.00"><infProt><chNFe>15260936275163000124550020000000011794431341</chNFe></infProt></protNFe>
</nfeProc>"#;

    #[test]
    fn le_nota_completa() {
        let n = ler_nfe_xml(NFE).unwrap();
        assert_eq!(n.chave_acesso, "15260936275163000124550020000000011794431341");
        assert_eq!(n.numero, "1");
        assert_eq!(n.serie, "2");
        assert_eq!(n.data_emissao.as_deref(), Some("2026-09-11T12:00:00-03:00"));
        assert_eq!(n.valor_total, 476.0);
        assert_eq!(
            n.emitente,
            EmitenteXml {
                documento: "36275163000124".into(),
                nome: "Distribuidora & Bebidas LTDA".into(),
                fantasia: Some("Distri".into()),
                ie: Some("150000000".into()),
                fone: Some("9132220000".into()),
                endereco: Some("Rua A, 10, Centro".into()),
                municipio: Some("Belém".into()),
                uf: Some("PA".into()),
            }
        );
        assert_eq!(n.itens.len(), 2);
        assert_eq!(
            n.itens[0],
            ItemXml {
                codigo: Some("001".into()),
                ean: Some("07891234567895".into()),
                nome: "CERVEJA LATA 350ML".into(),
                quantidade: 10.0,
                custo_unitario: 45.9,
                ncm: Some("02203000".into()),
                cfop: Some("5405".into()),
                unidade: "CX".into(),
            }
        );
        assert_eq!(n.itens[1].ean, None);
        assert_eq!(n.itens[1].nome, "GELO 5KG");
        assert_eq!(n.itens[1].unidade, "un");
        assert_eq!(valor_nf(NFE), Some(476.0));
        assert!(parece_xml_nfe(NFE));
    }

    #[test]
    fn nfe_sem_nfeproc_e_campos_faltando() {
        let xml = r#"<NFe><infNFe><ide><dEmi>2010-01-02</dEmi></ide><emit><CPF>529.982.247-25</CPF></emit>
            <det><prod><xProd>X</xProd></prod></det></infNFe></NFe>"#;
        let n = ler_nfe_xml(xml).unwrap();
        assert_eq!(n.chave_acesso, "");
        assert_eq!(n.emitente.documento, "52998224725");
        assert_eq!(n.emitente.nome, "Fornecedor sem nome");
        assert_eq!(n.emitente.endereco, None);
        assert_eq!(n.data_emissao.as_deref(), Some("2010-01-02"));
        assert_eq!(n.valor_total, 0.0);
        assert_eq!(n.itens[0].quantidade, 0.0);
        assert_eq!(valor_nf(xml), None);
    }

    #[test]
    fn resumo_e_invalidos() {
        let res = r#"<resNFe versao="1.01"><chNFe>1</chNFe><vNF>123.45</vNF></resNFe>"#;
        assert_eq!(valor_nf(res), Some(123.45));
        assert!(!parece_xml_nfe(res));
        assert_eq!(
            ler_nfe_xml(res).unwrap_err().to_string(),
            "O arquivo não parece ser um XML válido de NF-e."
        );
        assert!(ler_nfe_xml("<a><b></a>").is_err());
    }
}
