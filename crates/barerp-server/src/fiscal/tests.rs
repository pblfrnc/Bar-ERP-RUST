//! Testes das rotas fiscais: roteador inteiro com `oneshot`, banco em memória e uma Focus simulada em
//! localhost. Nenhum teste fala com a Focus, a SEFAZ ou a BrasilAPI de verdade.

use std::io::Read;
use std::path::PathBuf;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use barerp_core::ids;
use barerp_db::notas::{self, FiscalSettings, RegistroEmitida, RegistroRecebida};
use barerp_db::Db;
use chrono::Datelike;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{router, FiscalState};

const CNPJ: &str = "36275163000124";
const CHAVE: &str = "15260936275163000124550020000000011794431341";

struct Ambiente {
    app: Router,
    focus: MockServer,
    db: Db,
    pasta: PathBuf,
}

impl Drop for Ambiente {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.pasta);
    }
}

async fn ambiente(com_token: bool) -> Ambiente {
    let db = Db::open_in_memory().await.unwrap();
    if com_token {
        let cfg = FiscalSettings {
            api_token: Some("tok".into()),
            cnpj: Some("36.275.163/0001-24".into()),
            csc_id: Some("1".into()),
            csc_secret: Some("CSC".into()),
            crt: Some("1".into()),
            serie_nfce: Some("1".into()),
            serie_nfe: Some("2".into()),
            uf: Some("PA".into()),
            nome_fantasia: Some("Bar do Zé".into()),
            ..Default::default()
        };
        notas::salvar_config_fiscal(&db, &cfg).await.unwrap();
    }
    let focus = MockServer::start().await;
    let pasta = std::env::temp_dir().join(format!("barerp-fiscal-rotas-{}", ids::cuid()));
    let app = router(FiscalState::simulado(db.clone(), pasta.clone(), &focus.uri()));
    Ambiente { app, focus, db, pasta }
}

async fn chamar(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    (status, to_bytes(res.into_body(), usize::MAX).await.unwrap().to_vec())
}

async fn json_de(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let (s, b) = chamar(app, req).await;
    (s, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into_owned())))
}

fn post(uri: &str, corpo: Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .header("host", "192.168.0.10:3001")
        .body(Body::from(corpo.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).header("host", "192.168.0.10:3001").body(Body::empty()).unwrap()
}

fn multipart(uri: &str, metodo: &str, campos: &[(&str, Option<&str>, &[u8])]) -> Request<Body> {
    const B: &str = "----barerp";
    let mut corpo = Vec::new();
    for (nome, arquivo, conteudo) in campos {
        corpo.extend(format!("--{B}\r\nContent-Disposition: form-data; name=\"{nome}\"").as_bytes());
        if let Some(a) = arquivo {
            corpo.extend(format!("; filename=\"{a}\"\r\nContent-Type: application/octet-stream").as_bytes());
        }
        corpo.extend(b"\r\n\r\n");
        corpo.extend(*conteudo);
        corpo.extend(b"\r\n");
    }
    corpo.extend(format!("--{B}--\r\n").as_bytes());
    Request::builder()
        .method(metodo)
        .uri(uri)
        .header("content-type", format!("multipart/form-data; boundary={B}"))
        .body(Body::from(corpo))
        .unwrap()
}

fn item(nome: &str, ncm: &str) -> Value {
    json!({ "name": nome, "quantity": 2, "price": 10, "ncm": ncm, "cfop": "5102", "unit": "un" })
}

fn xml_nfe(chave: &str, vnf: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><nfeProc><NFe><infNFe Id="NFe{chave}"><ide><nNF>1</nNF><serie>2</serie><dhEmi>2026-09-11T12:00:00-03:00</dhEmi></ide>
        <emit><CNPJ>{CNPJ}</CNPJ><xNome>Distribuidora LTDA</xNome><enderEmit><xMun>Belém</xMun><UF>PA</UF></enderEmit></emit>
        <det nItem="1"><prod><cProd>7</cProd><cEAN>SEM GTIN</cEAN><xProd>GELO</xProd><NCM>22019000</NCM><CFOP>5102</CFOP><uCom>PC</uCom><qCom>3</qCom><vUnCom>5.5</vUnCom></prod></det>
        <total><ICMSTot><vNF>{vnf}</vNF></ICMSTot></total></infNFe></NFe></nfeProc>"#
    )
}

// ============================================================
// Emissão
// ============================================================

#[tokio::test]
async fn nfce_autorizada_grava_a_nota() {
    let a = ambiente(true).await;
    Mock::given(method("GET"))
        .and(path("/v2/nfce/nfce_venda1"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "codigo": "nao_encontrado" })))
        .mount(&a.focus)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/nfce"))
        .and(query_param("ref", "nfce_venda1"))
        .and(body_partial_json(json!({ "formas_pagamento": [{ "forma_pagamento": "03", "valor_pagamento": "18.00" }] })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "status": "autorizado", "ref": "nfce_venda1", "chave_nfe": "K65", "numero": 7, "serie": "1"
        })))
        .expect(1)
        .mount(&a.focus)
        .await;
    let corpo = json!({
        "saleKey": "venda1", "orderId": "bal7", "paymentMethod": "CREDIT_CARD",
        "items": [ { "name": "Cerveja", "quantity": 2, "price": 10, "discount": 2, "ncm": "22030000" } ]
    });
    let (s, r) = json_de(&a.app, post("/emit-nfce", corpo)).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "autorizado");
    assert_eq!(r["caminhoDanfe"], "http://192.168.0.10:3001/api/fiscal/danfe/nfce_venda1");
    let n = notas::buscar_emitida(&a.db, "nfce_venda1").await.unwrap().unwrap();
    assert_eq!((n.status.as_str(), n.numero.as_deref(), n.valor_total), ("autorizado", Some("7"), Some(18.0)));
}

#[tokio::test]
async fn nfce_sem_token_csc_ou_ncm() {
    let a = ambiente(false).await;
    let (s, r) = json_de(&a.app, post("/emit-nfce", json!({ "items": [item("X", "22030000")] }))).await;
    assert_eq!((s, r["error"].as_str().unwrap()), (StatusCode::BAD_REQUEST, "Token da API Fiscal não configurado. Vá nas Configurações Fiscais."));
    let (_, r) = json_de(&a.app, post("/emit-nfce", json!({ "items": [] }))).await;
    assert_eq!(r["error"], "Nenhum item adicionado para a nota.");

    let b = ambiente(true).await;
    let (s, r) = json_de(&b.app, post("/emit-nfce", json!({ "items": [item("Batata", "2004")] }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(r["error"].as_str().unwrap().contains("\"Batata\" está sem NCM válido"));
    let mut cfg = notas::config_fiscal(&b.db).await.unwrap();
    cfg.csc_secret = Some(String::new());
    notas::salvar_config_fiscal(&b.db, &cfg).await.unwrap();
    let (_, r) = json_de(&b.app, post("/emit-nfce", json!({ "items": [item("X", "22030000")] }))).await;
    assert!(r["error"].as_str().unwrap().starts_with("Para emitir NFC-e, preencha o Código CSC"));
}

#[tokio::test]
async fn nfe_recusada_fica_no_historico() {
    let a = ambiente(true).await;
    Mock::given(method("POST"))
        .and(path("/v2/nfe"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "status": "processando_autorizacao" })))
        .mount(&a.focus)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "erro_autorizacao", "mensagem_sefaz": "Rejeição 539" })))
        .mount(&a.focus)
        .await;
    let corpo = json!({ "orderId": "42", "customerCpf": "52998224725", "customerName": "Ana", "items": [item("Barril", "22030000")] });
    let (s, r) = json_de(&a.app, post("/emit-nfe", corpo)).await;
    assert_eq!((s, r["error"].as_str().unwrap()), (StatusCode::BAD_REQUEST, "SEFAZ recusou a NF-e: Rejeição 539"));
    let lista = notas::emitidas_recentes(&a.db, None).await.unwrap();
    assert_eq!(lista.len(), 1);
    assert!(lista[0].referencia.starts_with("nfe_42_"));
    assert_eq!((lista[0].status.as_str(), lista[0].valor_total), ("erro_autorizacao", Some(20.0)));
}

#[tokio::test]
async fn cancelamento_de_nfe_pelo_numero() {
    let a = ambiente(true).await;
    let reg = RegistroEmitida {
        referencia: "nfe_9_1000".into(),
        chave: Some(CHAVE.into()),
        numero: Some("1".into()),
        serie: Some("2".into()),
        ..Default::default()
    };
    notas::gravar_emitida(&a.db, &reg).await.unwrap();
    Mock::given(method("DELETE"))
        .and(path("/v2/nfe/nfe_9_1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "cancelado" })))
        .expect(1)
        .mount(&a.focus)
        .await;
    let (s, r) = json_de(&a.app, post("/cancel-nfe", json!({ "numero": "000000001", "justificativa": "Erro de digitação no pedido" }))).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["mensagem"], "NF-e cancelada com sucesso!");
    assert_eq!(notas::buscar_emitida(&a.db, CHAVE).await.unwrap().unwrap().status, "cancelado");

    let (s, r) = json_de(&a.app, post("/cancel-nfe", json!({ "numero": "1", "justificativa": "curta" }))).await;
    assert_eq!((s, r["error"].as_str().unwrap()), (StatusCode::BAD_REQUEST, "A justificativa deve ter no mínimo 15 caracteres (Regra da SEFAZ)."));
}

#[tokio::test]
async fn cancelamento_recusado_repassa_o_status() {
    let a = ambiente(true).await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "mensagem": "Prazo de cancelamento expirado" })))
        .mount(&a.focus)
        .await;
    let (s, r) = json_de(&a.app, post("/cancel-nfce", json!({ "referencia": "nfce_x", "justificativa": "Cliente desistiu da compra" }))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r["error"], "Erro ao cancelar nota: Prazo de cancelamento expirado");
    assert_eq!(r["details"]["mensagem"], "Prazo de cancelamento expirado");
}

// ============================================================
// Reimpressão e listas
// ============================================================

#[tokio::test]
async fn reimpressao_por_numero_sem_inventar_nota() {
    let a = ambiente(true).await;
    for (r, n, s) in [("nfce_a", "7", "1"), ("nfe_b", "3", "2")] {
        let reg = RegistroEmitida { referencia: r.into(), numero: Some(n.into()), serie: Some(s.into()), ..Default::default() };
        notas::gravar_emitida(&a.db, &reg).await.unwrap();
    }
    let (s, r) = json_de(&a.app, get("/reprint-by-number/7")).await;
    assert_eq!((s, r["nota"]["referencia"].as_str()), (StatusCode::OK, Some("nfce_a")));
    let (s, r) = json_de(&a.app, get("/reprint-by-number/3")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(r["error"].as_str().unwrap().contains("foi emitida como NF-e"));

    let (s, r) = json_de(&a.app, get("/nfe/reprint/3%20serie%202")).await;
    assert_eq!((s, r["nota"]["referencia"].as_str()), (StatusCode::OK, Some("nfe_b")));
    let (s, _) = json_de(&a.app, get("/nfe/reprint/3?serie=9")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // "1" não vira mais a nota do JUBAR, e chave desconhecida na Focus não é gravada como autorizada
    let (s, _) = json_de(&a.app, get("/nfe/reprint/1")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = json_de(&a.app, get(&format!("/nfe/reprint/{CHAVE}"))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(notas::buscar_emitida(&a.db, CHAVE).await.unwrap().is_none());

    // canceláveis e recentes
    let (_, r) = json_de(&a.app, get("/cancelable-notes")).await;
    assert_eq!(r[0]["referencia"], "nfce_a");
    assert_eq!(r[0]["minutesRemaining"], 30);
    let (_, r) = json_de(&a.app, get("/nfe/cancelable-notes")).await;
    assert_eq!((r[0]["referencia"].as_str(), r[0]["hoursRemaining"].as_i64()), (Some("nfe_b"), Some(24)));
    let (_, r) = json_de(&a.app, get("/recent-notes?tipo=nfce")).await;
    assert_eq!(r.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn danfe_html_ajustado_e_pdf_pela_chave() {
    let a = ambiente(true).await;
    Mock::given(method("GET"))
        .and(path("/v2/nfce/nfce_a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "autorizado", "caminho_danfe": "/notas/nfce_a.html" })))
        .mount(&a.focus)
        .await;
    Mock::given(method("GET"))
        .and(path("/notas/nfce_a.html"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<html><head></head><body><div id=\"qr-code0\"></div><script>new QRCode(x, { text: \"https://sefa/q?p=1\" });</script></body></html>",
            "text/html",
        ))
        .mount(&a.focus)
        .await;
    let (s, b) = chamar(&a.app, get("/danfe/nfce_a")).await;
    let html = String::from_utf8(b).unwrap();
    assert_eq!(s, StatusCode::OK);
    assert!(html.contains("create-qr-code/?size=100x100&data=https%3A%2F%2Fsefa%2Fq%3Fp%3D1"));
    assert!(html.contains("max-width: 80mm !important;"));

    Mock::given(method("GET"))
        .and(path("/v2/nfe/nfe_9"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "mensagem": "Nota fiscal não encontrada" })))
        .mount(&a.focus)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfe/{CHAVE}.pdf")))
        .respond_with(ResponseTemplate::new(200).set_body_raw(b"%PDF-1.4".to_vec(), "application/pdf"))
        .mount(&a.focus)
        .await;
    notas::gravar_emitida(&a.db, &RegistroEmitida { referencia: "nfe_9".into(), chave: Some(CHAVE.into()), ..Default::default() })
        .await
        .unwrap();
    let (s, b) = chamar(&a.app, get("/danfe/nfe_9")).await;
    assert_eq!((s, b.as_slice()), (StatusCode::OK, &b"%PDF-1.4"[..]));
    let (s, b) = chamar(&a.app, get("/danfe/nfe_nada")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(String::from_utf8(b).unwrap().starts_with("Erro Focus NFe (404)"));
}

// ============================================================
// Notas recebidas
// ============================================================

#[tokio::test]
async fn bip_sem_xml_e_depois_com_xml() {
    let a = ambiente(true).await;
    Mock::given(method("POST"))
        .and(path(format!("/v2/nfes_recebidas/{CHAVE}/manifesto")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "ok" })))
        .mount(&a.focus)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfes_recebidas/{CHAVE}.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "processando", "nome_emitente": "Ambev" })))
        .up_to_n_times(1)
        .mount(&a.focus)
        .await;
    let (s, r) = json_de(&a.app, post("/bip-chave", json!({ "chave": CHAVE }))).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!((r["status"].as_str(), r["xmlDisponivel"].as_bool(), r["emitente"].as_str()), (Some("ciencia_registrada"), Some(false), Some("Ambev")));
    assert_eq!((r["numero"].as_str(), r["serie"].as_str()), (Some("1"), Some("2")));

    // a SEFAZ liberou o XML
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfes_recebidas/{CHAVE}.xml")))
        .respond_with(ResponseTemplate::new(200).set_body_string(xml_nfe(CHAVE, "16.50")))
        .mount(&a.focus)
        .await;
    let (_, r) = json_de(&a.app, post(&format!("/notas-recebidas/{CHAVE}/sync"), json!({}))).await;
    assert_eq!(r["xmlDisponivel"], true);
    let n = notas::buscar_recebida(&a.db, CHAVE).await.unwrap().unwrap();
    assert_eq!((n.status.as_str(), n.valor_total, n.emitente.as_deref()), ("recebida", Some(16.5), Some("Distribuidora LTDA")));
    assert!(n.tem_xml());
    let hoje = chrono::Local::now();
    let no_cofre = notas::caminho_cofre(&a.pasta.join("xml_vault"), notas::LadoCofre::Entrada, CHAVE, hoje.year(), hoje.month());
    assert!(no_cofre.unwrap().exists());

    // itens para a conferência (2º bip) vêm do XML salvo, sem chamar a Focus
    let (s, r) = json_de(&a.app, get(&format!("/parsed-by-chave/{CHAVE}"))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((r["items"][0]["name"].as_str(), r["items"][0]["unit"].as_str(), r["valorTotal"].as_f64()), (Some("GELO"), Some("PC"), Some(16.5)));
    assert!(r["vendor"]["id"].is_string());

    // finalizada não regride na sincronização
    let (_, r) = json_de(&a.app, post(&format!("/notas-pendentes/{CHAVE}/finalizar"), json!({}))).await;
    assert_eq!(r["success"], true);
    json_de(&a.app, post(&format!("/notas-recebidas/{CHAVE}/sync"), json!({}))).await;
    assert_eq!(notas::buscar_recebida(&a.db, CHAVE).await.unwrap().unwrap().status, "finalizada");

    let (s, b) = chamar(&a.app, get(&format!("/notas-recebidas/{CHAVE}/xml"))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8(b).unwrap().contains("<vNF>16.50</vNF>"));
}

#[tokio::test]
async fn sincronizacao_das_recebidas_e_importacao() {
    let a = ambiente(true).await;
    let outra = "15261036275163000124550020000000021794431341";
    Mock::given(method("GET"))
        .and(path("/v2/nfes_recebidas"))
        .and(query_param("cnpj", CNPJ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "chave_nfe": CHAVE, "nome_emitente": "Ambev", "valor_total": "10.00", "versao": 1 },
            { "chave_nfe": outra, "valor_total": "0", "versao": 2, "manifestacao_destinatario": "ciencia" }
        ])))
        .mount(&a.focus)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfes_recebidas/{CHAVE}.xml")))
        .respond_with(ResponseTemplate::new(200).set_body_string(xml_nfe(CHAVE, "16.50")))
        .mount(&a.focus)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/v2/nfes_recebidas/{CHAVE}/manifesto")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&a.focus)
        .await;
    let (_, r) = json_de(&a.app, get("/sync-nfe-recebidas")).await;
    assert_eq!((r["count"].as_u64(), r["updatedWithXml"].as_u64()), (Some(2), Some(1)));
    let (_, r) = json_de(&a.app, get("/sync-nfe-recebidas")).await;
    assert_eq!(r["count"], 0);

    let (_, r) = json_de(&a.app, get("/notas-pendentes?xmlStatus=com_xml")).await;
    assert_eq!((r["summary"]["total"].as_u64(), r["notas"].as_array().unwrap().len()), (Some(2), 1));
    let segunda = notas::buscar_recebida(&a.db, outra).await.unwrap().unwrap();
    assert_eq!((segunda.status.as_str(), segunda.emitente.as_deref()), ("pendente", Some("Fornecedor CNPJ 36.275.163/0001-24")));

    // entrada no estoque: cria o produto e finaliza a nota
    sqlx::query(r#"INSERT INTO "Category" ("id", "name") VALUES ('c', 'Cozinha')"#).execute(a.db.pool()).await.unwrap();
    let corpo = json!({
        "items": [ { "xmlItem": { "name": "GELO", "quantity": 3, "unitCost": 5.5, "unit": "PC" }, "action": "NEW", "categoryId": "c" } ],
        "chaveAcesso": CHAVE, "valorTotal": 16.5
    });
    let req = Request::post("/apply-import")
        .header("content-type", "application/json")
        .header("x-user-name", "Ana")
        .body(Body::from(corpo.to_string()))
        .unwrap();
    let (s, r) = json_de(&a.app, req).await;
    assert_eq!((s, r["results"]["created"].as_u64()), (StatusCode::OK, Some(1)));
    assert_eq!(notas::buscar_recebida(&a.db, CHAVE).await.unwrap().unwrap().status, "finalizada");
    let (s, r) = json_de(&a.app, post("/apply-import", json!({ "nada": 1 }))).await;
    assert_eq!((s, r["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Nenhum item para importar.")));
}

#[tokio::test]
async fn importacao_do_arquivo_xml() {
    let a = ambiente(false).await;
    let xml = xml_nfe(CHAVE, "16.50");
    let (s, r) = json_de(&a.app, multipart("/import-xml", "POST", &[("xml", Some("nota.xml"), xml.as_bytes())])).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!((r["accessKey"].as_str(), r["valorTotal"].as_f64(), r["vendor"]["cnpj"].as_str()), (Some(CHAVE), Some(16.5), Some(CNPJ)));
    assert_eq!(r["items"][0]["quantity"], 3.0);
    assert_eq!(notas::buscar_recebida(&a.db, CHAVE).await.unwrap().unwrap().status, "recebida");
    let (s, r) = json_de(&a.app, multipart("/import-xml", "POST", &[("xml", Some("x.xml"), b"<html/>")])).await;
    assert_eq!((s, r["error"].as_str()), (StatusCode::BAD_REQUEST, Some("O arquivo não parece ser um XML válido de NF-e.")));
    let (s, _) = json_de(&a.app, multipart("/import-xml", "POST", &[])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

// ============================================================
// Contador
// ============================================================

#[tokio::test]
async fn fechamento_do_mes_sem_xml_inventado() {
    let a = ambiente(true).await;
    let reg = |r: &str, chave: Option<&str>, xml_url: Option<String>| RegistroEmitida {
        referencia: r.into(),
        chave: chave.map(str::to_string),
        numero: Some("1".into()),
        data_emissao: Some("2026-09-05T10:00:00-03:00".into()),
        valor_total: Some(30.0),
        xml_url,
        ..Default::default()
    };
    let chave65 = "15260936275163000124650010000000071794431341";
    notas::gravar_emitida(&a.db, &reg("nfce_a", Some(chave65), Some(format!("{}/arquivos/a.xml", a.focus.uri())))).await.unwrap();
    notas::gravar_emitida(&a.db, &reg("nfe_b", None, None)).await.unwrap();
    let recebida = RegistroRecebida {
        chave: CHAVE.into(),
        numero: Some("1".into()),
        data_emissao: Some("2026-09-11".into()),
        xml_content: Some(xml_nfe(CHAVE, "16.50")),
        ..Default::default()
    };
    notas::gravar_recebida(&a.db, &recebida, None).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/arquivos/a.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<nfeProc>NFCe A</nfeProc>"))
        .mount(&a.focus)
        .await;

    let (s, b) = chamar(&a.app, get("/export-month?month=2026-09")).await;
    assert_eq!(s, StatusCode::OK);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(b)).unwrap();
    let nomes: Vec<String> = zip.file_names().map(str::to_string).collect();
    assert!(nomes.contains(&format!("NFCe_Emitidas/NFCe_1_{chave65}.xml")));
    assert!(nomes.contains(&format!("NFe_Recebidas/NFe_1_{CHAVE}.xml")));
    assert!(!nomes.iter().any(|n| n.starts_with("NFe_Emitidas/")));
    let mut ler = |nome: &str| {
        let mut s = String::new();
        zip.by_name(nome).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    assert!(ler("XMLS_FALTANDO.txt").contains("NFe_Emitidas/NFe_1_sem_chave.xml (referência nfe_b"));
    let csv = ler("Relatorio_Fiscal_2026-09.csv");
    assert!(csv.starts_with('\u{FEFF}'));
    assert!(csv.contains("Total de NF-e Emitidas (Modelo 55):;1;R$ 30,00"));
    assert!(csv.contains("Total de NF-e Recebidas (Entradas/Compras):;1;R$ 16,50"));

    let (_, r) = json_de(&a.app, get("/month-summary?month=2026-09")).await;
    assert_eq!((r["nfceCount"].as_u64(), r["recebidasTotal"].as_f64(), r["hasNotes"].as_bool()), (Some(1), Some(16.5), Some(true)));
    let (s, _) = json_de(&a.app, get("/month-summary?month=setembro")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

// ============================================================
// Configuração e consultas
// ============================================================

#[tokio::test]
async fn salvar_configuracao_cadastra_a_empresa() {
    let a = ambiente(true).await;
    Mock::given(method("POST"))
        .and(path("/v2/empresas"))
        .and(body_partial_json(json!({ "cnpj": CNPJ, "habilita_nfce": true, "numero": 100, "uf": "PA" })))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "codigo": "empresa_ja_cadastrada" })))
        .expect(1)
        .mount(&a.focus)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/v2/empresas/{CNPJ}")))
        .and(body_partial_json(json!({ "senha_certificado": "1234" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .expect(1)
        .mount(&a.focus)
        .await;
    let dados = json!({
        "apiToken": " tok ", "cnpj": "36.275.163/0001-24", "cscId": "1", "cscSecret": "CSC", "numero": "100",
        "uf": "pa", "proximoNumeroNfce": "15", "razaoSocial": "Bar LTDA"
    })
    .to_string();
    let req = multipart(
        "/settings",
        "PUT",
        &[("settings", None, dados.as_bytes()), ("certificado", Some("a1.pfx"), b"PFX"), ("certPassword", None, b"1234")],
    );
    let (s, r) = json_de(&a.app, req).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!((r["apiToken"].as_str(), r["proximoNumeroNfce"].as_i64()), (Some("tok"), Some(15)));
    // o que não veio na tela ficou como estava
    assert_eq!((r["serieNfe"].as_str(), r["nomeFantasia"].as_str()), (Some("2"), Some("Bar do Zé")));

    let (_, r) = json_de(&a.app, get("/establishment")).await;
    assert_eq!((r["nomeFantasia"].as_str(), r["razaoSocial"].as_str()), (Some("Bar do Zé"), Some("Bar LTDA")));
}

#[tokio::test]
async fn consultas_sefaz_ncm_cnpj() {
    let a = ambiente(true).await;
    Mock::given(method("GET"))
        .and(path("/v2/nfce/status"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&a.focus)
        .await;
    Mock::given(method("GET")).and(path("/ping-sefaz")).respond_with(ResponseTemplate::new(200)).mount(&a.focus).await;
    let (_, r) = json_de(&a.app, get("/sefaz-status")).await;
    assert_eq!((r["online"].as_bool(), r["autorizador"].as_str()), (Some(true), Some("SEFAZ PA (SVRS)")));
    assert_eq!(r["motivoStatus"], "SEFAZ Ativa (Token Focus NFe não autorizado para Homologação)");

    Mock::given(method("GET"))
        .and(path("/v2/ncms/22030000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "codigo": "22030000", "descricao": "Cervejas de malte" })))
        .mount(&a.focus)
        .await;
    let (_, r) = json_de(&a.app, get("/ncm/2203.00.00")).await;
    assert_eq!((r["valido"].as_bool(), r["descricao"].as_str()), (Some(true), Some("Cervejas de malte")));
    let (s, r) = json_de(&a.app, get("/ncm/1111")).await;
    assert_eq!((s, r["valido"].as_bool()), (StatusCode::NOT_FOUND, Some(false)));

    Mock::given(method("GET"))
        .and(path(format!("/api/cnpj/v1/{CNPJ}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "razao_social": "BAR LTDA", "uf": "PA" })))
        .mount(&a.focus)
        .await;
    let (_, r) = json_de(&a.app, get(&format!("/consulta-cnpj/{CNPJ}"))).await;
    assert_eq!((r["razaoSocial"].as_str(), r["uf"].as_str()), (Some("BAR LTDA"), Some("PA")));
    let (s, _) = json_de(&a.app, get("/consulta-cnpj/123")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (_, r) = json_de(&a.app, get("/focus-backups")).await;
    assert_eq!((r["ok"].as_bool(), r["backups"].as_array().map(Vec::len)), (Some(true), Some(0)));
}

#[tokio::test]
async fn pesquisa_de_ncm_pelo_nome() {
    let a = ambiente(true).await;
    Mock::given(method("GET"))
        .and(path("/v2/ncms"))
        .and(query_param("descricao", "cerveja"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Total-Count", "3")
                .set_body_json(json!([{ "codigo": "22030000", "descricao_completa": "Cervejas de malte." }])),
        )
        .mount(&a.focus)
        .await;
    let (s, r) = json_de(&a.app, get("/ncms?q=cerveja")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((r["total"].as_u64(), r["itens"][0]["codigo"].as_str()), (Some(3), Some("22030000")));
    assert_eq!(r["itens"][0]["descricao"], "Cervejas de malte.");

    // Termo curto demais não vai à Focus
    let (s, _) = json_de(&a.app, get("/ncms?q=ce")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = json_de(&a.app, get("/ncms?q=2")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Sem token: avisa onde cadastrar
    let sem = ambiente(false).await;
    let (s, r) = json_de(&sem.app, get("/ncms?q=cerveja")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(r["error"].as_str().unwrap_or("").contains("token da Focus"));
}

#[tokio::test]
async fn xml_emitida_do_cofre_ou_da_focus() {
    let a = ambiente(true).await;
    let reg = RegistroEmitida { referencia: "nfce_a".into(), chave: Some(CHAVE.into()), ..Default::default() };
    notas::gravar_emitida(&a.db, &reg).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/v2/nfce/nfce_a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "autorizado", "caminho_xml_nota_fiscal": "/arquivos/a.xml" })))
        .expect(1)
        .mount(&a.focus)
        .await;
    Mock::given(method("GET"))
        .and(path("/arquivos/a.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<nfeProc>A</nfeProc>"))
        .expect(1)
        .mount(&a.focus)
        .await;
    // 1ª vez baixa da Focus e guarda; 2ª vez vem do cofre
    for _ in 0..2 {
        let (s, b) = chamar(&a.app, get("/xml-emitida/nfce_a")).await;
        assert_eq!((s, b.as_slice()), (StatusCode::OK, &b"<nfeProc>A</nfeProc>"[..]));
    }
    let (s, _) = chamar(&a.app, get("/xml-emitida/nada")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // chamada dentro do processo, como o app nativo faz
    let st = FiscalState::simulado(a.db.clone(), a.pasta.clone(), &a.focus.uri());
    let r = super::chamar(&st, "GET", "/establishment", super::Corpo::Nada).await;
    assert_eq!((r.status, r.json()["nomeFantasia"].as_str()), (200, Some("Bar do Zé")));
    let xml = xml_nfe(CHAVE, "1.00");
    let r = super::chamar(&st, "POST", "/import-xml", super::Corpo::Multipart(vec![super::CampoMultipart::arquivo("xml", "n.xml", xml.into_bytes())])).await;
    assert!(r.ok(), "{}", r.erro());
    let r = super::chamar(&st, "POST", "/emit-nfce", super::Corpo::Json(json!({ "items": [] }))).await;
    assert_eq!((r.status, r.erro().as_str()), (400, "Nenhum item adicionado para a nota."));
}
