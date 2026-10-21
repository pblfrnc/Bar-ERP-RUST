//! Cliente da Focus contra um servidor simulado em localhost. Nenhum teste fala com a Focus de verdade.

use std::time::Duration;

use barerp_fiscal::{
    montar_nfce, montar_nfe, Ambiente, Emissao, Emitente, FiscalError, FocusClient, ItemNota, Modelo, Venda,
};
use chrono::Utc;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CNPJ: &str = "36275163000124";
/// base64("tok:")
const AUTH: &str = "Basic dG9rOg==";

async fn cliente(server: &MockServer) -> FocusClient {
    FocusClient::com_base_url(&server.uri(), Ambiente::Homologacao, " tok ", "36.275.163/0001-24")
        .unwrap()
        .com_esperas(Duration::ZERO, Duration::ZERO, Duration::from_millis(200))
}

fn venda() -> Venda {
    Venda {
        itens: vec![ItemNota { nome: "Cerveja".into(), quantidade: 2.0, preco: 10.0, ncm: "22030000".into(), ..Default::default() }],
        forma_pagamento: "PIX".into(),
        ..Default::default()
    }
}

fn emitente() -> Emitente {
    Emitente { cnpj: CNPJ.into(), ..Default::default() }
}

#[tokio::test]
async fn nfce_autorizada_direto() {
    let server = MockServer::start().await;
    let nota = montar_nfce(&emitente(), &venda(), Utc::now()).unwrap();
    Mock::given(method("POST"))
        .and(path("/v2/nfce"))
        .and(query_param("ref", "nfce_v1"))
        .and(query_param("cnpj_emitente", CNPJ))
        .and(query_param("dry_run", "0"))
        .and(header("authorization", AUTH))
        .and(body_json(&nota.payload))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "status": "autorizado", "ref": "nfce_v1", "chave_nfe": "NFe123", "numero": 7, "serie": "1",
            "caminho_xml_nota_fiscal": "/arquivos/x.xml", "caminho_danfe": "/notas/nfce_v1.html"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let r = cliente(&server).await.emitir_nfce("nfce_v1", &nota.payload, false).await.unwrap();
    let Emissao::Autorizada(n) = r else { panic!("esperava autorizada: {r:?}") };
    assert_eq!(n.referencia, "nfce_v1");
    assert_eq!(n.numero.as_deref(), Some("7"));
    assert_eq!(n.caminho_danfe.as_deref(), Some("/notas/nfce_v1.html"));
}

#[tokio::test]
async fn nfce_ja_autorizada_nao_reenvia() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/nfce/nfce_venda1"))
        .and(query_param("cnpj_emitente", CNPJ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "autorizado", "ref": "nfce_venda1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(500)).expect(0).mount(&server).await;

    let p = montar_nfce(&emitente(), &venda(), Utc::now()).unwrap().payload;
    let r = cliente(&server).await.emitir_nfce("nfce_venda1", &p, true).await.unwrap();
    assert!(matches!(r, Emissao::Autorizada(_)));
}

#[tokio::test]
async fn nfce_com_erro_anterior_reenvia_e_processa() {
    let server = MockServer::start().await;
    // consulta prévia: erro_autorizacao; depois do envio, a consulta devolve autorizado
    Mock::given(method("GET"))
        .and(path("/v2/nfce/nfce_v2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "erro_autorizacao"})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/nfce"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"status": "processando_autorizacao", "ref": "nfce_v2"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/nfce/nfce_v2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "autorizado", "ref": "nfce_v2", "chave_nfe": "K"})))
        .mount(&server)
        .await;

    let p = montar_nfce(&emitente(), &venda(), Utc::now()).unwrap().payload;
    let r = cliente(&server).await.emitir_nfce("nfce_v2", &p, true).await.unwrap();
    let Emissao::Autorizada(n) = r else { panic!("{r:?}") };
    assert_eq!(n.chave.as_deref(), Some("K"));
}

#[tokio::test]
async fn nfce_continua_processando() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"status": "processando_autorizacao"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "processando_autorizacao"})))
        .mount(&server)
        .await;
    let p = montar_nfce(&emitente(), &venda(), Utc::now()).unwrap().payload;
    let r = cliente(&server).await.emitir_nfce("nfce_v3", &p, false).await.unwrap();
    assert!(matches!(r, Emissao::Processando { referencia, .. } if referencia == "nfce_v3"));
}

#[tokio::test]
async fn nfce_recusada_pela_sefaz_e_pela_focus() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(query_param("ref", "rej"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "erro_autorizacao", "mensagem_sefaz": "Rejeição: NCM inexistente"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(query_param("ref", "403"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"codigo": "permissao_negada", "mensagem": "x"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(query_param("ref", "422"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"erros": [{"campo": "cfop", "mensagem": "inválido"}]})))
        .mount(&server)
        .await;
    let c = cliente(&server).await;
    let p = montar_nfce(&emitente(), &venda(), Utc::now()).unwrap().payload;

    let e = c.emitir_nfce("rej", &p, false).await.unwrap_err();
    assert_eq!(e.to_string(), "SEFAZ recusou a NFC-e: Rejeição: NCM inexistente");
    assert!(matches!(e, FiscalError::Sefaz { .. }));

    let e = c.emitir_nfce("403", &p, false).await.unwrap_err();
    assert!(e.to_string().starts_with("Focus NFe (403): CNPJ 36275163000124 não autorizado pela Focus NFe"));
    assert!(e.to_string().contains("no ambiente de Homologação"));

    let e = c.emitir_nfce("422", &p, false).await.unwrap_err();
    assert_eq!(e.to_string(), "Focus NFe (422): [cfop] inválido");
    assert!(matches!(e, FiscalError::Focus { http_status: 422, .. }));
}

#[tokio::test]
async fn nfe_consulta_ate_autorizar() {
    let server = MockServer::start().await;
    let nota = montar_nfe(&emitente(), &venda(), "nfe_9_1000", Utc::now()).unwrap();
    Mock::given(method("POST"))
        .and(path("/v2/nfe"))
        .and(query_param("ref", "nfe_9_1000"))
        .and(query_param_is_missing("cnpj_emitente"))
        .and(body_json(&nota.payload))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"status": "processando_autorizacao"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/nfe/nfe_9_1000"))
        .and(query_param("completa", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "processando_autorizacao"})))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/nfe/nfe_9_1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "autorizado", "chave_nfe": "K55", "numero": "3"})))
        .mount(&server)
        .await;

    let r = cliente(&server).await.emitir_nfe("nfe_9_1000", &nota.payload).await.unwrap();
    let Emissao::Autorizada(n) = r else { panic!("{r:?}") };
    assert_eq!((n.referencia.as_str(), n.chave.as_deref()), ("nfe_9_1000", Some("K55")));
}

#[tokio::test]
async fn nfe_denegada_nao_e_sucesso() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"status": "processando_autorizacao"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "denegado"})))
        .mount(&server)
        .await;
    let p = montar_nfe(&emitente(), &venda(), "r", Utc::now()).unwrap().payload;
    let e = cliente(&server).await.emitir_nfe("r", &p).await.unwrap_err();
    assert!(matches!(e, FiscalError::Sefaz { status, .. } if status == "denegado"));
}

#[tokio::test]
async fn cancelamento_carta_e_inutilizacao() {
    let server = MockServer::start().await;
    let just = "Venda lançada em duplicidade";
    Mock::given(method("DELETE"))
        .and(path("/v2/nfce/nfce_a%20b"))
        .and(body_json(json!({"justificativa": just})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "cancelado"})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v2/nfe/nfe_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "erro_cancelamento", "mensagem_sefaz": "Prazo expirado"})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v2/nfe/nfe_2"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"codigo": "nao_encontrado", "mensagem": "Nota não encontrada"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/nfe/nfe_1/carta_correcao"))
        .and(body_json(json!({"correcao": "Corrige o endereço do destinatário"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "autorizado"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/nfce/inutilizacao"))
        .and(body_json(json!({"cnpj": CNPJ, "serie": "1", "numero_inicial": 10, "numero_final": 12, "justificativa": just})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "autorizado"})))
        .mount(&server)
        .await;
    let c = cliente(&server).await;

    assert_eq!(c.cancelar(Modelo::Nfce, "nfce_a b", &format!("  {just} ")).await.unwrap()["status"], "cancelado");
    assert_eq!(
        c.cancelar(Modelo::Nfe, "nfe_1", just).await.unwrap_err().to_string(),
        "Erro ao cancelar NF-e: Prazo expirado"
    );
    let e = c.cancelar(Modelo::Nfe, "nfe_2", just).await.unwrap_err();
    assert_eq!(e.to_string(), "Erro ao cancelar NF-e: Nota não encontrada");
    assert!(matches!(e, FiscalError::Focus { http_status: 404, .. }));
    assert!(matches!(c.cancelar(Modelo::Nfe, "nfe_1", "curta").await, Err(FiscalError::Validacao(_))));

    c.carta_correcao("nfe_1", " Corrige o endereço do destinatário ").await.unwrap();
    c.inutilizar(Modelo::Nfce, "1", 10, 12, just).await.unwrap();
    assert!(matches!(c.inutilizar(Modelo::Nfe, "", 1, 2, just).await, Err(FiscalError::Validacao(_))));
}

#[tokio::test]
async fn status_da_sefaz() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/nfce/status"))
        .and(query_param("uf", "PA"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "", "codigo_status": 107, "motivo_status": "Serviço em Operação", "tempo_medio": "1"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/nfce/status"))
        .and(query_param("uf", "SP"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let c = cliente(&server).await;
    let s = c.status_sefaz(" pa").await.unwrap();
    assert!(s.online);
    assert_eq!((s.status.as_str(), s.codigo_status.as_str(), s.tempo_medio), ("online", "107", 1.0));
    let e = c.status_sefaz("SP").await.unwrap_err();
    assert_eq!(e.to_string(), "Token Focus NFe não autorizado para Homologação");
}

fn pagina(inicio: usize, n: usize) -> serde_json::Value {
    json!((inicio..inicio + n)
        .map(|i| json!({
            "chave_nfe": format!("{:044}", i),
            "nome_emitente": "Fornecedor",
            "documento_emitente": "36.275.163/0001-24",
            "valor_total": "10.50",
            "numero": i,
            "versao": i
        }))
        .collect::<Vec<_>>())
}

#[tokio::test]
async fn notas_recebidas_paginadas() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/nfes_recebidas"))
        .and(query_param("cnpj", CNPJ))
        .and(query_param_is_missing("versao"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pagina(1, 100)).insert_header("X-Max-Version", "100"))
        .expect(1)
        .mount(&server)
        .await;
    // sem cabeçalho: a próxima versão sai do maior "versao" dos itens
    Mock::given(method("GET"))
        .and(path("/v2/nfes_recebidas"))
        .and(query_param("versao", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pagina(101, 100)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/nfes_recebidas"))
        .and(query_param("versao", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"chave": "1"}, {"nome_emitente": "sem chave"}])))
        .expect(1)
        .mount(&server)
        .await;

    let lista = cliente(&server).await.listar_nfes_recebidas().await.unwrap();
    assert_eq!(lista.aviso, None);
    assert_eq!(lista.notas.len(), 201); // a nota sem chave é ignorada
    let n = &lista.notas[0];
    assert_eq!(n.chave, format!("{:044}", 1));
    assert_eq!(n.documento_emitente.as_deref(), Some(CNPJ));
    assert_eq!(n.valor_total, 10.5);
    assert_eq!(n.numero.as_deref(), Some("1"));
}

#[tokio::test]
async fn notas_recebidas_param_no_teto_e_no_erro() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param_is_missing("versao"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pagina(1, 100)).insert_header("X-Max-Version", "5"))
        .mount(&server)
        .await;
    // versão que não avança encerra; depois disso, erro da Focus vira aviso com o que já veio
    Mock::given(method("GET"))
        .and(query_param("versao", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pagina(1, 100)).insert_header("X-Max-Version", "5"))
        .expect(1)
        .mount(&server)
        .await;
    let lista = cliente(&server).await.listar_nfes_recebidas().await.unwrap();
    assert_eq!(lista.notas.len(), 200);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param_is_missing("versao"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pagina(1, 100)).insert_header("X-Max-Version", "100"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("versao", "100"))
        .respond_with(ResponseTemplate::new(500).set_body_string("falhou"))
        .mount(&server)
        .await;
    let lista = cliente(&server).await.listar_nfes_recebidas().await.unwrap();
    assert_eq!(lista.notas.len(), 100);
    assert_eq!(lista.aviso.as_deref(), Some("Focus NFe respondeu 500 ao listar notas recebidas (versao=100): falhou"));
}

#[tokio::test]
async fn notas_recebidas_teto_de_paginas() {
    let server = MockServer::start().await;
    // a Focus sempre devolve página cheia com versão maior: o laço para em 100 páginas
    Mock::given(method("GET"))
        .respond_with(move |req: &wiremock::Request| {
            let v: usize = req
                .url
                .query_pairs()
                .find(|(k, _)| k == "versao")
                .map(|(_, v)| v.parse().unwrap())
                .unwrap_or(0);
            ResponseTemplate::new(200).set_body_json(pagina(v + 1, 100)).insert_header("X-Max-Version", (v + 100).to_string())
        })
        .expect(100)
        .mount(&server)
        .await;
    let lista = cliente(&server).await.listar_nfes_recebidas().await.unwrap();
    assert_eq!(lista.notas.len(), 100 * 100);
}

#[tokio::test]
async fn manifesto_e_xml_recebida() {
    let server = MockServer::start().await;
    let chave = "15260936275163000124550020000000011794431341";
    let xml = "<?xml version=\"1.0\"?><nfeProc><NFe><infNFe/></NFe></nfeProc>";
    Mock::given(method("POST"))
        .and(path(format!("/v2/nfes_recebidas/{chave}/manifesto")))
        .and(body_json(json!({"tipo": "ciencia"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
        .expect(1)
        .mount(&server)
        .await;
    // .xml ainda não liberado; .json aponta para o arquivo
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfes_recebidas/{chave}.xml")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfes_recebidas/{chave}.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"caminho_xml_nota_fiscal": "/arquivos/nota.xml"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/arquivos/nota.xml"))
        .and(header("authorization", AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_string(xml))
        .mount(&server)
        .await;

    let c = cliente(&server).await;
    c.manifestar_ciencia(chave).await.unwrap();
    let r = c.baixar_xml_recebida(chave).await.unwrap();
    assert_eq!(r.xml.as_deref(), Some(xml));
    assert!(r.dados.is_some());
    assert_eq!(c.baixar_xml_recebida("1".repeat(44).as_str()).await, None);
}

#[tokio::test]
async fn danfe_pdf_pela_chave() {
    let server = MockServer::start().await;
    let chave = "15260936275163000124550020000000011794431341";
    Mock::given(method("GET"))
        .and(path(format!("/arquivos/{CNPJ}/202609/DANFEs/{chave}.pdf")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfe/{chave}.pdf")))
        .and(header("authorization", AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_raw(b"%PDF-1.4".to_vec(), "application/pdf"))
        .mount(&server)
        .await;
    let c = cliente(&server).await;
    assert_eq!(c.baixar_danfe_pdf_por_chave(chave).await.as_deref(), Some(&b"%PDF-1.4"[..]));
    assert_eq!(c.baixar_danfe_pdf_por_chave("123").await, None);
}

#[tokio::test]
async fn baixar_so_manda_token_para_a_focus() {
    let server = MockServer::start().await;
    let outro = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/danfe.html"))
        .and(header("authorization", AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<html/>", "text/html"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/s3/x.xml"))
        .respond_with(move |req: &wiremock::Request| {
            assert!(req.headers.get("authorization").is_none(), "token vazou para outro host");
            ResponseTemplate::new(200).set_body_string("<NFe/>")
        })
        .mount(&outro)
        .await;
    let c = cliente(&server).await;
    let a = c.baixar("danfe.html").await.unwrap();
    assert_eq!((a.content_type.as_str(), a.bytes.as_slice()), ("text/html", &b"<html/>"[..]));
    let b = c.baixar(&format!("{}/s3/x.xml", outro.uri())).await.unwrap();
    assert_eq!(b.bytes, b"<NFe/>");
    assert!(matches!(c.baixar("/nada").await, Err(FiscalError::Focus { http_status: 404, .. })));
}

#[tokio::test]
async fn xml_recebida_pelo_endpoint_legado() {
    let server = MockServer::start().await;
    let chave = "15260936275163000124550020000000011794431341";
    Mock::given(method("GET"))
        .and(path(format!("/v2/nfes_recebidas/{chave}/xml")))
        .respond_with(ResponseTemplate::new(200).set_body_string("<nfeProc/>"))
        .mount(&server)
        .await;
    let r = cliente(&server).await.baixar_xml_recebida(chave).await.unwrap();
    assert_eq!((r.xml.as_deref(), r.dados), (Some("<nfeProc/>"), None));
}

#[tokio::test]
async fn requisicao_generica() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/nfce/nfce_1/email"))
        .and(header("authorization", AUTH))
        .and(body_json(json!({"emails": ["a@b.com"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;
    let r = cliente(&server)
        .await
        .requisitar(barerp_fiscal::Method::POST, "/v2/nfce/nfce_1/email", Some(&json!({"emails": ["a@b.com"]})))
        .await
        .unwrap();
    assert_eq!((r.http_status, r.corpo["ok"].as_bool()), (200, Some(true)));
}

#[tokio::test]
async fn cnpj_publico_e_ping() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/cnpj/v1/{CNPJ}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "razao_social": "BAR LTDA", "nome_fantasia": "BAR", "cep": "66000000",
            "descricao_tipo_de_logradouro": "AVENIDA", "logradouro": "BRASIL", "numero": "10",
            "ddd_telefone_1": "", "ddd_telefone_2": "9132220000", "cnae_fiscal": 5611204
        })))
        .mount(&server)
        .await;
    let d = barerp_fiscal::publico::consultar_cnpj(&server.uri(), "36.275.163/0001-24").await.unwrap();
    assert_eq!((d.logradouro.as_str(), d.telefone.as_str(), d.cnae.clone()), ("AVENIDA BRASIL", "9132220000", json!(5611204)));
    let e = barerp_fiscal::publico::consultar_cnpj(&server.uri(), "11222333000181").await.unwrap_err();
    assert!(matches!(e, FiscalError::Focus { http_status: 404, .. }));

    Mock::given(method("GET")).and(path("/fora")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    Mock::given(method("GET")).and(path("/no-ar")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    let urls = [format!("{}/fora", server.uri()), format!("{}/no-ar", server.uri())];
    assert!(barerp_fiscal::publico::ping(&urls).await.is_some());
    assert_eq!(barerp_fiscal::publico::ping(&urls[..1]).await, None);
}

#[tokio::test]
async fn cep_pela_brasilapi_ou_viacep() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/cep/v1/66000000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "street": "Av. Brasil", "neighborhood": "Umarizal", "city": "Belém", "state": "PA" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ws/01001000/json/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "logradouro": "Praça da Sé", "bairro": "Sé", "localidade": "São Paulo", "uf": "SP" })))
        .mount(&server)
        .await;
    let u = server.uri();
    let e = barerp_fiscal::publico::consultar_cep(&u, &u, "66000-000").await.unwrap();
    assert_eq!((e.logradouro.as_str(), e.municipio.as_str()), ("Av. Brasil", "Belém"));
    let e = barerp_fiscal::publico::consultar_cep(&u, &u, "01001-000").await.unwrap();
    assert_eq!((e.logradouro.as_str(), e.uf.as_str()), ("Praça da Sé", "SP"));
    assert_eq!(barerp_fiscal::publico::consultar_cep(&u, &u, "123").await, None);
}
