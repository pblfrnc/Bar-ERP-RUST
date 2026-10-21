use super::*;
use barerp_core::ids;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EAN: &str = "7896045506040";

#[test]
fn inferencia_na_ordem_do_node() {
    assert_eq!(infer_tax("Guaraná Antarctica 350ml", None, &[]).ncm, "22021000", "refrigerante antes de cerveja");
    let c = infer_tax("Heineken Long Neck", None, &[]);
    assert_eq!((c.ncm, c.cest, c.cfop, c.code_prefix), ("22030000", Some("03.001.00"), "5405", "CERV"));
    assert_eq!(infer_tax("Hambúrguer artesanal", None, &[]).kds_station, "KITCHEN");
    assert_eq!(infer_tax("X-Burguer", None, &[]).kds_station, "NONE", "como no Node: burguer não casa com burger");
    assert_eq!(infer_tax("Algo 500 ML", None, &[]).code_prefix, "BAR");
    assert_eq!(infer_tax("Produto", Some("Marca"), &["bebidas"]).kds_station, "BAR", "categoria com bebida");
    let g = infer_tax("Parafuso", None, &[]);
    assert_eq!((g.ncm, g.kds_station, g.code_prefix), ("21069090", "NONE", "PRD"));
    assert_eq!(infer_tax("Original", None, &[]).code_prefix, "GIN", "como no Node: /gin/ sem fronteira");
}

#[test]
fn item_certo_da_nota() {
    let xml = format!(
        "<NFe><emit><CNPJ>1</CNPJ><xNome>Distribuidora Zé</xNome></emit><dest><xNome>Bar</xNome></dest>\
         <det nItem=\"1\"><prod><cProd>A1</cProd><cEAN>111</cEAN><xProd>Outro produto</xProd></prod></det>\
         <det nItem=\"2\"><prod><cProd>B2</cProd><cEAN>{EAN}</cEAN><xProd>Cerveja Lata</xProd><vUnCom>3.25</vUnCom></prod></det>\
         <detPag><tPag>01</tPag></detPag></NFe>"
    );
    let item = det_with_ean(&xml, EAN).unwrap();
    assert_eq!(tag(item, "xProd"), Some("Cerveja Lata"));
    assert_eq!(emit_name(&xml), Some("Distribuidora Zé"));
    assert!(det_with_ean(&xml, "999").is_none());
}

async fn db() -> Db {
    let db = Db::open_in_memory().await.unwrap();
    for (name, start) in [("Cozinha", 1001), ("Bar", 5001)] {
        sqlx::query(r#"INSERT INTO "Category" ("id","name","codeStart") VALUES (?,?,?)"#)
            .bind(ids::cuid())
            .bind(name)
            .bind(start)
            .execute(db.pool())
            .await
            .unwrap();
    }
    db
}

async fn off(body: serde_json::Value) -> (MockServer, Vec<String>) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/{EAN}.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let urls = vec![format!("{}/v2/{{ean}}.json", server.uri())];
    (server, urls)
}

#[tokio::test]
async fn codigo_curto_e_produto_ja_cadastrado() {
    let db = db().await;
    let cat = Catalog::with_urls(db.clone(), vec![]);
    let r = cat.lookup("12-34", LookupMode::All).await.unwrap();
    assert!(!r.found && r.message.contains("pelo menos 7 dígitos"));

    let cat_id: String = sqlx::query_scalar(r#"SELECT "id" FROM "Category" LIMIT 1"#).fetch_one(db.pool()).await.unwrap();
    sqlx::query(r#"INSERT INTO "Product" ("id","name","ean","price","categoryId","updatedAt") VALUES ('p','Brahma',?,8,?,0)"#)
        .bind(EAN)
        .bind(&cat_id)
        .execute(db.pool())
        .await
        .unwrap();
    let r = cat.lookup(EAN, LookupMode::All).await.unwrap();
    assert_eq!((r.found, r.source, r.name.as_deref()), (true, Some(Source::LocalProduct), Some("Brahma")));
    // "Puxar das Notas" não olha o cadastro
    let r = cat.lookup(EAN, LookupMode::Xml).await.unwrap();
    assert!(!r.found && r.message.contains("não foi encontrado em nenhuma Nota Fiscal"));
}

#[tokio::test]
async fn nota_recebida_com_custo_e_cfop_de_saida() {
    let db = db().await;
    let xml = format!(
        "<emit><xNome>Ambev</xNome></emit><det nItem=\"1\"><prod><cProd>778</cProd><cEAN>{EAN}</cEAN>\
         <xProd> Skol Lata 350ml </xProd><NCM>22030000</NCM><CFOP>1403</CFOP><uCom>UN</uCom><vUnCom>2.5</vUnCom></prod></det>"
    );
    sqlx::query(r#"INSERT INTO "NotaRecebida" ("id","chave","emitente","xmlContent","createdAt") VALUES ('n','c','Ambev SA',?,0)"#)
        .bind(&xml)
        .execute(db.pool())
        .await
        .unwrap();
    let r = Catalog::with_urls(db.clone(), vec![]).lookup(EAN, LookupMode::Xml).await.unwrap();
    assert_eq!((r.found, r.source), (true, Some(Source::NotaFiscalXml)));
    assert_eq!((r.name.as_deref(), r.code.as_deref(), r.supplier.as_deref()), (Some("Skol Lata 350ml"), Some("778"), Some("Ambev")));
    assert_eq!((r.cost_price, r.suggested_price), (Some(2.5), Some(5.5)));
    assert_eq!((r.cfop.as_deref(), r.cest.as_deref(), r.unit.as_deref()), (Some("5405"), Some("03.001.00"), Some("un")));
    let bar: String = sqlx::query_scalar(r#"SELECT "id" FROM "Category" WHERE "name" = 'Bar'"#).fetch_one(db.pool()).await.unwrap();
    assert_eq!(r.suggested_category_id, Some(bar));
    assert!(r.message.contains("Custo real: R$ 2.50"));
}

#[tokio::test]
async fn catalogo_global_completa_nome_e_codigo_da_faixa() {
    let db = db().await;
    let (_server, urls) = off(serde_json::json!({
        "status": 1,
        "product": { "product_name_pt": "Cerveja Heineken", "brands": "Heineken, Heineken Brasil", "quantity": "330",
                     "categories_tags": ["en:beers"] }
    }))
    .await;
    let r = Catalog::with_urls(db, urls).lookup(EAN, LookupMode::Global).await.unwrap();
    assert_eq!((r.found, r.source), (true, Some(Source::OpenFoodFacts)));
    assert_eq!(r.name.as_deref(), Some("Cerveja Heineken 330ml"));
    assert_eq!((r.brand.as_deref(), r.supplier.as_deref()), (Some("Heineken"), Some("Heineken")));
    assert_eq!((r.ncm.as_deref(), r.code.as_deref()), (Some("22030000"), Some("5001")));
    assert_eq!(r.description.as_deref(), Some("Embalagem 330ml"));
}

#[tokio::test]
async fn nada_encontrado_vira_modelo() {
    let db = db().await;
    let (_server, urls) = off(serde_json::json!({ "status": 0 })).await;
    let cat = Catalog::with_urls(db, urls);
    let r = cat.lookup(EAN, LookupMode::Global).await.unwrap();
    assert!(!r.found && r.message.contains("Catálogo Global/Nacional GTIN"));
    let r = cat.lookup(EAN, LookupMode::All).await.unwrap();
    assert!(!r.found);
    assert_eq!((r.code.as_deref(), r.ncm.as_deref(), r.kds_station.as_deref()), (Some("PRD-6040"), Some("22029900"), Some("BAR")));
}
