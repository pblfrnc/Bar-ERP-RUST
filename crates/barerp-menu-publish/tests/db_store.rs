//! Ponta a ponta: bar.db em memória + adaptador `DbMenuStore` + site falso (wiremock).

use barerp_core::ids;
use barerp_db::digital_menu as dm;
use barerp_db::{Db, DbTime};
use barerp_menu_publish::db_store::{save_product_image, DbMenuStore};
use barerp_menu_publish::{image_hash, MenuPublisher, MenuStore};
use serde_json::{json, Value};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const JPEG: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];

async fn exec(db: &Db, sql: &str, binds: &[&str]) {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(db.pool()).await.unwrap();
}

/// Bebida com adicional e foto, porção da cozinha e um produto fora do cardápio.
async fn seed(db: &Db) -> (String, String) {
    let n = DbTime::now().0.timestamp_millis().to_string();
    let n = n.as_str();
    let (cat, beer, fries, hidden, group) = (
        ids::cuid(),
        ids::cuid(),
        ids::cuid(),
        ids::cuid(),
        ids::cuid(),
    );
    exec(db, r#"INSERT INTO "Category" ("id","name","icon","sortOrder") VALUES (?, 'Bebidas', 'Beer', 0)"#, &[&cat]).await;
    let product = r#"INSERT INTO "Product" ("id","name","price","categoryId","stock","trackStock","kdsStation","showInMenu","createdAt","updatedAt")
                     VALUES (?,?,?,?,?,1,?,?,?,?)"#;
    exec(
        db,
        product,
        &[&beer, "Chopp", "12", &cat, "0", "BAR", "1", n, n],
    )
    .await;
    exec(
        db,
        product,
        &[&fries, "Fritas", "29.9", &cat, "0", "KITCHEN", "1", n, n],
    )
    .await;
    exec(
        db,
        product,
        &[&hidden, "Oculto", "5", &cat, "10", "BAR", "0", n, n],
    )
    .await;
    exec(db, r#"INSERT INTO "ModifierGroup" ("id","name","minSelect","maxSelect","createdAt","updatedAt") VALUES (?, 'Copo', 0, 1, ?, ?)"#, &[&group, n, n]).await;
    exec(
        db,
        r#"INSERT INTO "ModifierOption" ("id","groupId","name","price") VALUES (?,?, 'Caneca', 2)"#,
        &[&ids::cuid(), &group],
    )
    .await;
    exec(
        db,
        r#"INSERT INTO "ProductModifierGroup" ("productId","groupId") VALUES (?,?)"#,
        &[&beer, &group],
    )
    .await;
    exec(db, r#"INSERT INTO "FiscalSettings" ("id","nomeFantasia","updatedAt") VALUES ('default', ' Bar do Zé ', ?)"#, &[n]).await;
    (beer, fries)
}

#[tokio::test]
async fn liga_publica_e_grava_no_banco() {
    let db = Db::open_in_memory().await.unwrap();
    let (beer, _) = seed(&db).await;
    let hash = save_product_image(&db, &beer, JPEG.to_vec()).await.unwrap();
    assert_eq!(hash, image_hash(&JPEG));

    let site = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true })))
        .mount(&site)
        .await;
    let publisher = MenuPublisher::new(DbMenuStore::new(db.clone()));
    let r = publisher
        .save_settings(&json!({ "enabled": true, "siteUrl": site.uri() }))
        .await
        .unwrap();
    assert_eq!(r.publish_error, None);
    assert_eq!(r.status.default_title, "Bar do Zé");

    // Gravado no bar.db
    let row = dm::settings(&db).await.unwrap();
    let store_id = row.store_id.clone().unwrap();
    assert!(row.enabled && row.last_hash.is_some() && row.last_published_at.is_some());
    assert_eq!(
        dm::images(&db, std::slice::from_ref(&beer)).await.unwrap()[0]
            .published_hash
            .as_deref(),
        Some(hash.as_str())
    );

    // Corpo publicado: só o que está no cardápio, com adicional, foto e esgotado do bar
    let reqs = site.received_requests().await.unwrap();
    let menu_put = reqs
        .iter()
        .find(|r| r.url.path() == format!("/api/menu/{store_id}"))
        .unwrap();
    let body: Value = serde_json::from_slice(&menu_put.body).unwrap();
    let products = body["products"].as_array().unwrap();
    assert_eq!(
        products
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Chopp", "Fritas"]
    );
    assert_eq!(products[0]["soldOut"], true);
    assert_eq!(products[1]["soldOut"], false, "cozinha nunca esgota");
    assert_eq!(
        products[0]["groups"][0]["options"][0],
        json!({ "name": "Caneca", "price": 2 })
    );
    assert_eq!(
        products[0]["image"],
        format!("/api/menu/{store_id}/img/{hash}")
    );
    assert_eq!(body["store"]["name"], "Bar do Zé");

    // Nada mudou: não envia de novo
    let before = site.received_requests().await.unwrap().len();
    publisher.publish(false).await.unwrap();
    assert_eq!(site.received_requests().await.unwrap().len(), before);

    // Tirou o produto do cardápio: muda o hash e publica
    dm::set_menu_visibility(&db, &[], std::slice::from_ref(&beer))
        .await
        .unwrap();
    publisher.publish(false).await.unwrap();
    assert!(site.received_requests().await.unwrap().len() > before);
    let source = DbMenuStore::new(db.clone()).menu_source().await.unwrap();
    assert_eq!(source.products.len(), 1);
}
