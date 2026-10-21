//! Rotas chamadas com `oneshot` num banco em memória, conferindo o formato que o Node devolvia.

use axum::body::Body as HttpBody;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use barerp_core::ids;
use barerp_db::{Db, DbTime};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::events::kds_new_order;
use crate::{PrintRequest, Server};

struct Fixture {
    server: Server,
    app: Router,
    table1: String,
    table2: String,
    beer: String,
    burger: String,
    bacon: String,
}

async fn exec(db: &Db, sql: &str, binds: &[&str]) {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(db.pool()).await.unwrap();
}

async fn fixture() -> Fixture {
    let db = Db::open_in_memory().await.unwrap();
    let now = DbTime::now().0.timestamp_millis().to_string();
    let n = now.as_str();
    let (cat, table1, table2, beer, burger, group, rare, bacon) =
        (ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid(), ids::cuid());

    exec(&db, r#"INSERT INTO "Category" ("id","name","sortOrder") VALUES (?, 'Bar', 1)"#, &[&cat]).await;
    for (t, num) in [(&table1, "1"), (&table2, "2")] {
        exec(&db, r#"INSERT INTO "Table" ("id","number","createdAt","updatedAt") VALUES (?,?,?,?)"#, &[t, num, n, n]).await;
    }
    let product = r#"INSERT INTO "Product" ("id","name","price","categoryId","stock","trackStock","hasBoxPrice","boxQuantity","kdsStation","brand","createdAt","updatedAt")
                     VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"#;
    exec(&db, product, &[&beer, "Cerveja Lata", "8", &cat, "240", "1", "0", "12", "BAR", "Heineken", n, n]).await;
    exec(&db, product, &[&burger, "Hambúrguer", "32", &cat, "50", "0", "0", "24", "KITCHEN", "", n, n]).await;
    exec(&db, r#"INSERT INTO "ModifierGroup" ("id","name","minSelect","maxSelect","createdAt","updatedAt") VALUES (?, 'Ponto', 0, 2, ?, ?)"#, &[&group, n, n]).await;
    exec(&db, r#"INSERT INTO "ModifierOption" ("id","groupId","name","price","sortOrder") VALUES (?,?, 'Mal passado', 0, 0)"#, &[&rare, &group]).await;
    exec(&db, r#"INSERT INTO "ModifierOption" ("id","groupId","name","price","sortOrder") VALUES (?,?, 'Bacon', 5.5, 1)"#, &[&bacon, &group]).await;
    exec(&db, r#"INSERT INTO "ModifierOption" ("id","groupId","name","price","sortOrder","isActive") VALUES (?,?, 'Antigo', 1, 2, 0)"#, &[&ids::cuid(), &group]).await;
    exec(&db, r#"INSERT INTO "ProductModifierGroup" ("productId","groupId") VALUES (?,?)"#, &[&burger, &group]).await;

    let server = Server::new(db, std::env::temp_dir(), 3001);
    let app = server.router();
    Fixture { server, app, table1, table2, beer, burger, bacon }
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value, HeaderMap) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            HttpBody::from(b.to_string())
        }
        None => HttpBody::empty(),
    };
    let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value, headers)
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (s, v, _) = call(app, Method::GET, uri, None).await;
    (s, v)
}

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let (s, v, _) = call(app, Method::POST, uri, Some(body)).await;
    (s, v)
}

async fn put(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let (s, v, _) = call(app, Method::PUT, uri, Some(body)).await;
    (s, v)
}

/// Data como o `JSON.stringify` de um `Date`: `2026-10-18T15:27:10.588Z`.
fn is_iso(v: &Value) -> bool {
    let s = v.as_str().unwrap_or_default();
    s.len() == 24 && s.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(s).is_ok()
}

async fn open(f: &Fixture, table: &str) -> String {
    let (status, body) = post(&f.app, &format!("/api/tables/{table}/open"), json!({ "waiterName": "Ana", "customerCount": "3" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["order"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn health_and_headers() {
    let f = fixture().await;
    let (status, body, headers) = call(&f.app, Method::GET, "/api/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["port"], 3001);
    assert!(is_iso(&body["time"]));
    assert!(body["uptime"].is_number() && body["ips"].is_array());
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers.get("content-security-policy").is_none());

    let req = Request::get("/api/health").header("origin", "http://localhost").body(HttpBody::empty()).unwrap();
    let res = f.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.headers()["access-control-allow-origin"], "*");

    let (status, body) = get(&f.app, "/api/network/devices").await;
    assert_eq!((status, body), (StatusCode::OK, json!([])));
}

#[tokio::test]
async fn staff_login() {
    let f = fixture().await;
    let (_, list) = get(&f.app, "/api/staff/public-list").await;
    let admin = &list[0];
    assert_eq!(admin["name"], "Administrador");
    assert_eq!(admin.as_object().unwrap().len(), 3, "só id, name e role: {admin}");
    let id = admin["id"].as_str().unwrap();

    let (status, body) = post(&f.app, "/api/staff/login", json!({ "userId": id, "password": "0000" })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({ "error": "Senha incorreta. Tentativa 1 de 5.", "attempts": 1, "maxAttempts": 5, "lockedOut": false }));

    let (status, body) = post(&f.app, "/api/staff/login", json!({ "userId": id, "password": "1234" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["user"]["role"], "ADMIN");
    assert!(body["user"]["permissions"].is_array());
    assert!(body["user"].get("defaultPassword").is_none());

    let (status, body) = post(&f.app, "/api/staff/login", json!({ "userId": id })).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Usuário e senha são obrigatórios")));
}

#[tokio::test]
async fn tables_in_prisma_format() {
    let f = fixture().await;
    let (status, list) = get(&f.app, "/api/tables").await;
    assert_eq!(status, StatusCode::OK);
    let t = &list[0];
    assert_eq!((t["number"].clone(), t["status"].clone(), t["capacity"].clone()), (json!(1), json!("AVAILABLE"), json!(4)));
    assert_eq!(t["activeOrder"], Value::Null);
    assert_eq!(t["orders"], json!([]));
    assert!(is_iso(&t["createdAt"]) && t["openedAt"].is_null());

    let order_id = open(&f, &f.table1).await;
    let (_, list) = get(&f.app, "/api/tables").await;
    let t = &list[0];
    assert_eq!(t["status"], "OCCUPIED");
    assert_eq!(t["customerCount"], 3);
    assert_eq!(t["currentOrderId"], json!(order_id));
    let order = &t["activeOrder"];
    assert_eq!(order["orderNumber"], 101);
    assert_eq!(order["waiterName"], "Ana");
    assert_eq!(order["isServiceFeeActive"], false);
    assert_eq!(order["serviceFeeRate"], 0.1);
    assert_eq!((order["items"].clone(), order["payments"].clone()), (json!([]), json!([])));
    assert!(order.get("table").is_none());

    let (status, body) = post(&f.app, &format!("/api/tables/{}/open", f.table1), json!({})).await;
    assert_eq!((status, body), (StatusCode::CONFLICT, json!({ "error": "Mesa já está ocupada" })));
}

#[tokio::test]
async fn table_crud() {
    let f = fixture().await;
    let (status, t) = post(&f.app, "/api/tables", json!({ "number": "7", "capacity": 6 })).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!((t["name"].clone(), t["capacity"].clone(), t["section"].clone()), (json!("Mesa 7"), json!(6), json!("Salão Principal")));
    let id = t["id"].as_str().unwrap().to_string();

    let (status, body) = post(&f.app, "/api/tables", json!({ "number": 7 })).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Mesa 7 já existe")));

    let (status, body) = put(&f.app, &format!("/api/tables/{id}"), json!({ "number": 1 })).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Já existe uma mesa cadastrada com o número 1")));
    let (_, t) = put(&f.app, &format!("/api/tables/{id}"), json!({ "number": 8, "section": " Deck " })).await;
    assert_eq!((t["number"].clone(), t["section"].clone(), t["name"].clone()), (json!(8), json!("Deck"), json!("Mesa 7")));

    let (_, body) = put(&f.app, "/api/tables/batch/section", json!({ "tableIds": [f.table1, f.table2], "section": "VIP" })).await;
    assert_eq!(body, json!({ "success": true, "count": 2 }));
    let (_, body) = post(&f.app, "/api/tables/sections/rename", json!({ "oldSection": "VIP", "newSection": "Varanda" })).await;
    assert_eq!(body, json!({ "success": true, "count": 2 }));

    open(&f, &f.table1).await;
    let (status, body, _) = call(&f.app, Method::DELETE, &format!("/api/tables/{}", f.table1), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Apenas mesas livres podem ser excluídas")));
    let (_, body, _) = call(&f.app, Method::DELETE, &format!("/api/tables/{id}"), None).await;
    assert_eq!(body, json!({ "success": true, "message": "Mesa 8 excluída com sucesso" }));
}

#[tokio::test]
async fn order_flow() {
    let f = fixture().await;
    let order_id = open(&f, &f.table1).await;
    let items = json!({ "items": [
        { "productId": f.beer, "quantity": 2 },
        { "productId": f.burger, "quantity": 1, "notes": "sem cebola", "modifierOptionIds": [f.bacon] },
    ]});
    let (status, body) = post(&f.app, &format!("/api/orders/{order_id}/items"), items).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let added = body["addedItems"].as_array().unwrap();
    assert_eq!(added.len(), 2);
    assert_eq!(added[0]["product"]["name"], "Cerveja Lata");
    assert_eq!(added[0]["kdsStatus"], "PENDING");
    assert!(is_iso(&added[0]["addedAt"]));
    // O item guarda só a observação digitada; a cozinha recebe a junção com os adicionais
    assert_eq!(added[1]["notes"], "sem cebola");
    assert_eq!(body["order"]["subtotal"], 16.0 + 37.5);
    assert_eq!(body["order"]["table"]["number"], 1);
    assert_eq!(body["order"]["items"].as_array().unwrap().len(), 2);

    let ids: Vec<String> = added.iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
    let kds = kds_new_order(&f.server.handle.state, &order_id, &ids).await.unwrap().unwrap();
    assert_eq!(kds["tableName"], "Mesa 1");
    assert_eq!(kds["tableNumber"], 1);
    assert_eq!(kds["waiterName"], "Ana");
    assert_eq!(kds["orderNumber"], 101);
    assert_eq!(kds["items"][1]["notes"], "Bacon | sem cebola");
    assert_eq!(kds["items"][1]["product"]["kdsStation"], "KITCHEN");

    let mut events = f.server.handle().subscribe();
    put(&f.app, &format!("/api/orders/{order_id}/service-fee"), json!({ "active": false })).await;
    let ev = events.try_recv().unwrap();
    assert_eq!((ev.name.as_str(), ev.payload["orderId"].as_str()), ("order:updated", Some(order_id.as_str())));

    let (status, order) = get(&f.app, &format!("/api/orders/{order_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(order["items"][0]["product"]["name"], "Hambúrguer", "itens mais novos primeiro");
    assert_eq!(order["payments"], json!([]));

    let (_, order) = put(&f.app, &format!("/api/orders/{order_id}/service-fee"), json!({})).await;
    assert_eq!(order["isServiceFeeActive"], true);
    assert_eq!(order["serviceFee"], 5.35);
    assert_eq!(order["total"], 58.85);
    let (_, order) = put(&f.app, &format!("/api/orders/{order_id}/discount"), json!({ "discount": "8.85" })).await;
    assert_eq!(order["discount"], 8.85);
    assert_eq!(order["total"], 50);

    let (status, order, _) = call(&f.app, Method::DELETE, &format!("/api/orders/{order_id}/items/{}", ids[0]), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(order["items"].as_array().unwrap().len(), 1);
    assert!(order.get("payments").is_none(), "o Node não inclui pagamentos aqui");

    let (status, body) = get(&f.app, "/api/orders/nao-existe").await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, json!({ "error": "Comanda não encontrada" })));
    let (status, body) = post(&f.app, &format!("/api/orders/{order_id}/items"), json!({ "items": [] })).await;
    assert_eq!((status, body), (StatusCode::BAD_REQUEST, json!({ "error": "Nenhum item informado" })));
}

#[tokio::test]
async fn closing_transfer_and_merge() {
    let f = fixture().await;
    open(&f, &f.table1).await;

    // Sem corpo e sem Content-Type, como o app manda
    let (status, table, _) = call(&f.app, Method::POST, &format!("/api/tables/{}/request-closing", f.table1), None).await;
    assert_eq!((status, table["status"].clone()), (StatusCode::OK, json!("CLOSING")));
    let (_, table, _) = call(&f.app, Method::POST, &format!("/api/tables/{}/reopen", f.table1), None).await;
    assert_eq!(table["status"], "OCCUPIED");

    let (status, body) = post(&f.app, &format!("/api/tables/{}/transfer", f.table1), json!({ "targetTableId": f.table1 })).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Mesa de origem e destino devem ser diferentes")));
    let (_, body) = post(&f.app, &format!("/api/tables/{}/transfer", f.table1), json!({ "targetTableId": f.table2 })).await;
    assert_eq!(body, json!({ "success": true, "message": "Mesa 1 transferida para Mesa 2" }));

    open(&f, &f.table1).await;
    let (_, body) = post(&f.app, &format!("/api/tables/{}/merge", f.table2), json!({ "secondTableId": f.table1 })).await;
    assert_eq!(body, json!({ "success": true, "message": "Mesa 1 unida à Mesa 2" }));
}

#[tokio::test]
async fn products_catalog() {
    let f = fixture().await;
    let (status, list) = get(&f.app, "/api/products").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 2);
    let burger = list.as_array().unwrap().iter().find(|p| p["id"] == json!(f.burger)).unwrap();
    assert_eq!(burger["category"]["name"], "Bar");
    assert_eq!(burger["supplierRel"], Value::Null);
    assert_eq!(burger["components"], json!([]));
    assert_eq!(burger["imageHash"], Value::Null);
    assert_eq!(burger["trackStock"], false);
    assert_eq!(burger["price"], 32);
    let options = burger["modifierGroups"][0]["group"]["options"].as_array().unwrap();
    assert_eq!(options.iter().map(|o| o["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Mal passado", "Bacon"]);

    let (_, found) = get(&f.app, "/api/products?search=heine").await;
    assert_eq!(found[0]["name"], "Cerveja Lata");
    let (_, found) = get(&f.app, "/api/products?search=100%25").await;
    assert_eq!(found, json!([]));

    let (_, cats) = get(&f.app, "/api/products/categories/all").await;
    assert_eq!(cats[0]["_count"], json!({ "products": 2 }));
    let (_, groups) = get(&f.app, "/api/products/modifier-groups").await;
    assert_eq!(groups[0]["options"].as_array().unwrap().len(), 3);
    assert_eq!(groups[0]["_count"], json!({ "products": 1 }));

    let (_, active) = get(&f.app, "/api/products/promotions/active").await;
    assert!(is_iso(&active["serverTime"]) && active["serverTimezoneOffset"].is_number());
    assert_eq!(active["promotions"], json!([]));

    let (status, body) = get(&f.app, &format!("/api/products/{}/image", f.beer)).await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, json!({ "error": "Produto sem foto" })));
}

#[tokio::test]
async fn receipt_on_native_printer() {
    let f = fixture().await;
    let order_id = open(&f, &f.table1).await;
    let uri = format!("/api/orders/{order_id}/print-receipt");

    let (status, body) = post(&f.app, &uri, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().starts_with("Nenhum computador do caixa conectado"));

    let (tx, mut rx) = tokio::sync::mpsc::channel::<PrintRequest>(1);
    f.server.handle().set_local_printer(tx);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let ok = req.order["table"]["number"] == 1 && req.requested_by.as_deref() == Some("Ana");
            req.reply.send(if ok { Ok(()) } else { Err("pedido errado".into()) }).ok();
        }
    });
    let (status, body) = post(&f.app, &uri, json!({ "requestedBy": "Ana" })).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "ok": true })));
}

#[tokio::test]
async fn cash_routes_flow() {
    let f = fixture().await;

    // 1. Caixa inicialmente fechado
    let (status, body) = get(&f.app, "/api/cash/current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["isOpen"], false);
    assert_eq!(body["shift"], Value::Null);

    // 2. Abrir caixa
    let (status, shift) = post(&f.app, "/api/cash/open", json!({
        "initialBalance": 100.0,
        "openedBy": "Caixa 1",
        "notes": "Abertura teste"
    })).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(shift["status"], "OPEN");
    assert_eq!(shift["initialBalance"], 100.0);

    // 3. Suprimento e Sangria
    let (status, tx) = post(&f.app, "/api/cash/transaction", json!({
        "type": "SUPPLY",
        "amount": 50.0,
        "reason": "Troco"
    })).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(tx["amount"], 50.0);

    let (status, tx2) = post(&f.app, "/api/cash/transaction", json!({
        "type": "WITHDRAWAL",
        "amount": 20.0,
        "reason": "Sangria malote"
    })).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(tx2["amount"], 20.0);

    // 4. Consultar caixa atual
    let (status, current) = get(&f.app, "/api/cash/current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["isOpen"], true);
    assert_eq!(current["summary"]["expectedCashInDrawer"], 130.0);

    // 5. Fechar caixa (fechamento cego)
    let (status, closed) = post(&f.app, "/api/cash/close", json!({
        "finalCashCount": 130.0,
        "closedBy": "Caixa 1"
    })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["success"], true);
    assert_eq!(closed["report"]["difference"], 0.0);

    // 6. Reabrir último turno
    let (status, reopened) = post(&f.app, "/api/cash/reopen-last", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reopened["success"], true);
    assert_eq!(reopened["shift"]["status"], "OPEN");

    // 7. Histórico
    let (status, hist) = get(&f.app, "/api/cash/history").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!hist.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn order_pay_route() {
    let f = fixture().await;
    let order_id = open(&f, &f.table1).await;
    let items = json!({ "items": [
        { "productId": f.beer, "quantity": 2 }, // 2 x 8 = 16.0
    ]});
    let (status, _) = post(&f.app, &format!("/api/orders/{order_id}/items"), items).await;
    assert_eq!(status, StatusCode::CREATED);

    // 1. Pagar sem caixa aberto -> 403 Forbidden
    let (status, _) = post(&f.app, &format!("/api/orders/{order_id}/pay"), json!({
        "payments": [{ "amount": 10.0, "method": "CASH" }]
    })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 2. Abrir caixa
    post(&f.app, "/api/cash/open", json!({ "initialBalance": 50.0 })).await;

    // 3. Pagamento parcial (Haver R$ 6,00)
    let (status, body) = post(&f.app, &format!("/api/orders/{order_id}/pay"), json!({
        "payments": [{ "amount": 6.0, "method": "CASH" }]
    })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["isFullyPaid"], false);
    assert_eq!(body["order"]["paidAmount"], 6.0);
    assert_eq!(body["order"]["status"], "OPEN");

    // 4. Quitar restante (R$ 10,00) com PIX e fechar
    let (status, body2) = post(&f.app, &format!("/api/orders/{order_id}/pay"), json!({
        "payments": [{ "amount": 10.0, "method": "PIX" }],
        "closeOrder": true
    })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body2["success"], true);
    assert_eq!(body2["isFullyPaid"], true);
    assert_eq!(body2["order"]["paidAmount"], 16.0);
    assert_eq!(body2["order"]["status"], "PAID");

    // Mesa liberada
    let (_, tables) = get(&f.app, "/api/tables").await;
    let t = tables.as_array().unwrap().iter().find(|t| t["id"] == json!(f.table1)).unwrap();
    assert_eq!(t["status"], "AVAILABLE");
}

#[tokio::test]
async fn pay_retry_with_same_request_id_is_charged_once() {
    let f = fixture().await;
    let order_id = open(&f, &f.table1).await;
    post(&f.app, &format!("/api/orders/{order_id}/items"), json!({ "items": [{ "productId": f.beer, "quantity": 2 }] })).await;
    post(&f.app, "/api/cash/open", json!({ "initialBalance": 50.0 })).await;

    // O celular reenvia o mesmo POST quando o Wi-Fi oscila; os dois chegam juntos
    let uri = format!("/api/orders/{order_id}/pay");
    let pay = json!({ "payments": [{ "amount": 6.0, "method": "PIX" }], "requestId": "171-abc" });
    let ((s1, b1), (s2, b2)) = tokio::join!(post(&f.app, &uri, pay.clone()), post(&f.app, &uri, pay.clone()));
    assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK));
    assert_eq!(b1, b2, "o reenvio recebe a mesma resposta");
    let (_, order) = get(&f.app, &format!("/api/orders/{order_id}")).await;
    assert_eq!(order["paidAmount"], 6.0);
    assert_eq!(order["payments"].as_array().unwrap().len(), 1);

    // Outro clique (id novo) lança de novo; sem corpo JSON válido vem o erro no formato do Node
    let (_, body) = post(&f.app, &uri, json!({ "payments": [{ "amount": 4.0, "method": "PIX" }], "requestId": "172-def" })).await;
    assert_eq!(body["order"]["paidAmount"], 10.0);
    let (status, body, _) = call(&f.app, Method::POST, &uri, None).await;
    assert_eq!((status, body), (StatusCode::BAD_REQUEST, json!({ "error": "Nenhum pagamento fornecido" })));
}

#[tokio::test]
async fn order_quick_sale_route() {
    let f = fixture().await;

    // 1. Sem caixa aberto -> 403 Forbidden
    let sale_payload = json!({
        "items": [{ "productId": f.beer, "quantity": 2 }],
        "payment": { "method": "CASH", "cashTendered": 20.0 },
        "customerName": "Cliente Rápido",
        "discount": 1.0,
    });
    let (status, _) = post(&f.app, "/api/orders/quick-sale", sale_payload.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 2. Abrir caixa
    post(&f.app, "/api/cash/open", json!({ "initialBalance": 100.0 })).await;

    // 3. Concluir venda de balcão
    let (status, body) = post(&f.app, "/api/orders/quick-sale", sale_payload).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["success"], true);
    assert_eq!(body["change"], 5.0); // 16.0 - 1.0 = 15.0; 20.0 - 15.0 = 5.0
    assert_eq!(body["order"]["status"], "PAID");
    assert_eq!(body["order"]["table"], Value::Null);
    assert_eq!(body["order"]["total"], 15.0);
    assert_eq!(body["order"]["paidAmount"], 15.0);
}

#[tokio::test]
async fn kds_routes_flow() {
    let f = fixture().await;

    // 1. Abrir mesa 1 e lançar cerveja (BAR) e hambúrguer (KITCHEN)
    let order_id = open(&f, &f.table1).await;

    let items = json!([
        { "productId": f.beer, "quantity": 2 },
        { "productId": f.burger, "quantity": 1, "notes": "sem cebola" }
    ]);
    post(&f.app, &format!("/api/orders/{order_id}/items"), json!({ "items": items })).await;

    // 2. Fila KDS geral (ALL)
    let (status, kds_all) = get(&f.app, "/api/kds").await;
    assert_eq!(status, StatusCode::OK);
    let items_arr = kds_all.as_array().unwrap();
    assert_eq!(items_arr.len(), 2);
    assert_eq!(items_arr[0]["orderId"], order_id);
    assert!(items_arr[0]["order"]["table"]["number"].is_number());
    assert!(items_arr[0]["product"]["name"].is_string());

    // 3. Fila KDS do Bar
    let (_, kds_bar) = get(&f.app, "/api/kds?station=BAR").await;
    let bar_items = kds_bar.as_array().unwrap();
    assert_eq!(bar_items.len(), 1);
    assert_eq!(bar_items[0]["kdsStation"], "BAR");
    let beer_item_id = bar_items[0]["id"].as_str().unwrap();

    // 4. Atualizar status de item individual: PENDING -> PREPARING -> READY
    let (status, updated) = put(&f.app, &format!("/api/kds/items/{beer_item_id}/status"), json!({ "status": "PREPARING" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["kdsStatus"], "PREPARING");

    let (_, updated_ready) = put(&f.app, &format!("/api/kds/items/{beer_item_id}/status"), json!({ "status": "READY" })).await;
    assert_eq!(updated_ready["kdsStatus"], "READY");

    // 5. Ready-all da cozinha
    let (status, ready_res) = post(&f.app, &format!("/api/kds/orders/{order_id}/ready-all"), json!({ "station": "KITCHEN" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ready_res["success"], true);

    // 6. Batch status -> DELIVERED
    let (status, batch_res) = post(&f.app, &format!("/api/kds/orders/{order_id}/batch-status"), json!({ "status": "DELIVERED" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(batch_res["success"], true);

    // 7. Agora a fila de produção deve estar vazia (todos entregues)
    let (_, kds_after) = get(&f.app, "/api/kds").await;
    assert_eq!(kds_after.as_array().unwrap().len(), 0);
}



#[tokio::test]
async fn busy_port_reports_clear_error() {
    // Simula o BarERP antigo (Node) ocupando a porta
    let busy = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = busy.local_addr().unwrap().port();
    let server = Server::new(Db::open_in_memory().await.unwrap(), std::env::temp_dir(), port);
    let handle = server.handle();
    assert_eq!(handle.startup_error(), None);

    let err = server.serve().await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    let msg = handle.startup_error().unwrap();
    assert_eq!(msg, format!("A porta {port} já está em uso. Feche o BarERP antigo (Electron) para os celulares conectarem."));
    assert_eq!(err.to_string(), msg);
}

#[tokio::test]
async fn settings_license_and_printer() {
    // Nenhuma chamada ao painel de verdade: a revalidação em segundo plano bate numa porta fechada
    std::env::set_var("LICENSE_SERVER_URL", "http://127.0.0.1:9");
    let f = fixture().await;
    let (status, body) = get(&f.app, "/api/settings").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["isLicensed"], json!(false));
    assert_eq!(body["licenseStatus"], json!("UNLICENSED"));
    assert!(body["machineId"].as_str().is_some_and(|m| !m.is_empty()));
    assert_eq!(body["developerContact"]["pixKey"], json!("68.817.608/0001-47"));
    assert!(body.get("licenseKey").is_none());

    let (status, body) = post(&f.app, "/api/settings/license/offline", json!({ "key": "EXP-20991231-XXXXXXXX-00000000" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
    let (status, body) = post(&f.app, "/api/settings/license/offline", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("Chave de ativação é obrigatória."));

    let (status, body) = post(&f.app, "/api/settings/license/sync-remote", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["message"].as_str().unwrap().starts_with("Não foi possível conectar"));

    let (status, body) = post(&f.app, "/api/settings/printer", json!({ "cashierPrinter": " POS-80 ", "paperWidth": 58, "autoCut": false })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["settings"]["cashierPrinter"], json!("POS-80"));
    assert_eq!(body["settings"]["paperWidth"], json!(58.0));
    assert_eq!(body["settings"]["autoCut"], json!(false));
    let (_, body) = get(&f.app, "/api/settings/printer").await;
    assert_eq!(body["copies"], json!(1));
}

async fn delete_req(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (s, v, _) = call(app, Method::DELETE, uri, None).await;
    (s, v)
}

#[tokio::test]
async fn waiter_apk_routes() {
    let f = fixture().await;
    // Garçons (abrir mesa no celular)
    let (status, w) = post(&f.app, "/api/waiters", json!({ "name": " Ana ", "code": "01" })).await;
    assert_eq!(status, StatusCode::CREATED, "{w}");
    assert_eq!(w["commissionRate"], json!(0.10));
    let (status, dup) = post(&f.app, "/api/waiters", json!({ "name": "Ana" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(dup["error"], json!("Já existe um garçom cadastrado com este nome"));
    let id = w["id"].as_str().unwrap().to_string();
    let (_, upd) = put(&f.app, &format!("/api/waiters/{id}"), json!({ "active": false })).await;
    assert_eq!(upd["active"], json!(false));
    let (_, list) = get(&f.app, "/api/waiters").await;
    assert_eq!(list.as_array().unwrap().len(), 0);
    let (_, list) = get(&f.app, "/api/waiters?all=true").await;
    assert_eq!(list[0]["name"], json!("Ana"));
    let (_, del) = delete_req(&f.app, &format!("/api/waiters/{id}")).await;
    assert_eq!(del["message"], json!("Garçom excluído com sucesso"));

    // Clientes do fiado
    let (_, c) = post(&f.app, "/api/customers", json!({ "name": "Carlos", "phone": "91 99999-0000" })).await;
    assert_eq!(c["phone"], json!("91 99999-0000"));
    assert_eq!(c["creditTabBalance"], json!(0.0));
    let (_, list) = get(&f.app, "/api/customers").await;
    assert_eq!(list[0]["name"], json!("Carlos"));
    let stored: String = sqlx::query_scalar(r#"SELECT "phone" FROM "Customer""#).fetch_one(f.server.handle().state.db.pool()).await.unwrap();
    assert!(stored.starts_with("$enc$"), "telefone cifrado no banco: {stored}");

    // Perda (calote)
    let order = open(&f, &f.table1).await;
    let (_, _) = post(&f.app, &format!("/api/orders/{order}/items"), json!({ "items": [{ "productId": f.beer, "quantity": 2 }] })).await;
    let (status, loss) = post(&f.app, &format!("/api/orders/{order}/loss"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{loss}");
    assert_eq!(loss["lostAmount"], json!(16.0));
    let (status, again) = post(&f.app, &format!("/api/orders/{order}/loss"), json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(again["error"], json!("Comanda já fechada."));
    let (_, audit) = get(&f.app, "/api/audit-logs").await;
    assert_eq!(audit[0]["action"], json!("LOSS_CALOTE"));
}

#[tokio::test]
async fn admin_routes() {
    let f = fixture().await;
    // Funcionários
    let (status, s) = post(&f.app, "/api/staff", json!({ "name": "Bia", "password": "4321", "permissions": ["tables", "kds"] })).await;
    assert_eq!(status, StatusCode::CREATED, "{s}");
    assert_eq!(s["role"], json!("OPERADOR"));
    let id = s["id"].as_str().unwrap().to_string();
    let (_, upd) = put(&f.app, &format!("/api/staff/{id}"), json!({ "role": "CAIXA" })).await;
    assert_eq!((upd["role"].clone(), upd["permissions"].clone()), (json!("CAIXA"), json!(["tables", "kds"])));
    let (_, del) = delete_req(&f.app, &format!("/api/staff/{id}")).await;
    assert_eq!(del["success"], json!(true));

    // Fornecedores
    let (status, sup) = post(&f.app, "/api/suppliers", json!({ "name": "Ambev", "document": "07.526.557/0001-00" })).await;
    assert_eq!(status, StatusCode::CREATED, "{sup}");
    assert_eq!(sup["document"], json!("07526557000100"));
    assert_eq!(sup["_count"]["products"], json!(0));
    let (_, list) = get(&f.app, "/api/suppliers?search=amb").await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Produtos: criar, alterar parcial, foto, categoria, promoção, adicionais
    let (_, cats) = get(&f.app, "/api/products/categories/all").await;
    let cat = cats[0]["id"].as_str().unwrap().to_string();
    let (status, _) = post(&f.app, "/api/products", json!({ "name": "Caipirinha" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, p) = post(&f.app, "/api/products", json!({ "name": "Caipirinha", "price": 18, "categoryId": cat })).await;
    assert_eq!(status, StatusCode::CREATED, "{p}");
    let pid = p["id"].as_str().unwrap().to_string();
    assert_eq!(p["category"]["id"], json!(cat));
    let (status, p2) = put(&f.app, &format!("/api/products/{pid}"), json!({ "price": 20, "isActive": false })).await;
    assert_eq!(status, StatusCode::OK, "{p2}");
    assert_eq!((p2["price"].clone(), p2["isActive"].clone(), p2["name"].clone()), (json!(20), json!(false), json!("Caipirinha")));
    let (_, hist) = get(&f.app, &format!("/api/products/{pid}/price-history")).await;
    assert!(!hist.as_array().unwrap().is_empty());
    let (status, _) = post(&f.app, "/api/products/categories", json!({ "name": "Porções" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, promo) = post(
        &f.app,
        "/api/products/promotions",
        json!({ "name": "Happy Hour", "value": "10", "daysOfWeek": [1, 2], "startTime": "17:00", "endTime": "19:00", "productIds": [pid] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{promo}");
    let (status, bad) = post(&f.app, "/api/products/promotions", json!({ "name": "X", "value": 5, "daysOfWeek": [] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    let (status, g) = post(&f.app, "/api/products/modifier-groups", json!({ "name": "Gelo", "options": [{ "name": "Com gelo" }, { "name": "Sem gelo" }] })).await;
    assert_eq!(status, StatusCode::CREATED, "{g}");
    let (status, _) = put(&f.app, &format!("/api/products/{pid}/modifier-groups"), json!({ "groupIds": [g["id"]] })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get(&f.app, "/api/products/stock-report").await;
    assert_eq!(status, StatusCode::OK);

    // Dashboard
    let (status, d) = get(&f.app, "/api/dashboard?date=2026-01-01").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(d["totalTables"], json!(2));
    assert!(d["paymentMethodsBreakdown"].is_object());
}

#[tokio::test]
async fn import_old_system_xml() {
    let f = fixture().await;
    let xml = "<Tabelas><Produtos><produto><ativo>1</ativo><nome>REFRIGERANTE COCA 350ML</nome><preco_venda>6</preco_venda></produto></Produtos></Tabelas>";
    let boundary = "XYZ";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"xml\"; filename=\"tabelas.xml\"\r\nContent-Type: text/xml\r\n\r\n{xml}\r\n--{boundary}--\r\n"
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/import-xml/tabelas")
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(HttpBody::from(body))
        .unwrap();
    let res = f.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["resumo"]["produtos"]["importados"], json!(1));
    assert_eq!(v["resumo"]["produtos"]["bar"], json!(1));
}
