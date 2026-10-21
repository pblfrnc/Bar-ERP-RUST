use chrono::NaiveDate;

use super::*;
use crate::tables;

pub(crate) struct Fixture {
    pub db: Db,
    pub table1: String,
    pub table2: String,
    pub beer: String,
    pub caipirinha: String,
    pub cachaca: String,
    pub burger: String,
    pub rare: String,
    pub bacon: String,
}

pub(crate) fn noon() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

async fn exec(db: &Db, sql: &str, binds: &[&str]) {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(db.pool()).await.unwrap();
}

pub(crate) async fn fixture() -> Fixture {
    let db = Db::open_in_memory().await.unwrap();
    let now = DbTime::now().0.timestamp_millis().to_string();
    let n = now.as_str();
    let id = |_: &str| ids::cuid();
    let (cat, table1, table2) = (id("cat"), id("t1"), id("t2"));
    let (beer, caipirinha, cachaca, burger) = (id("b"), id("c"), id("ca"), id("bu"));
    let (group, rare, bacon) = (id("g"), id("r"), id("ba"));

    exec(&db, r#"INSERT INTO "Category" ("id","name") VALUES (?, 'Bar')"#, &[&cat]).await;
    for (t, num) in [(&table1, "1"), (&table2, "2")] {
        exec(&db, r#"INSERT INTO "Table" ("id","number","createdAt","updatedAt") VALUES (?,?,?,?)"#, &[t, num, n, n]).await;
    }
    let product = r#"INSERT INTO "Product" ("id","name","price","categoryId","stock","trackStock","hasBoxPrice","boxQuantity","boxPrice","kdsStation","createdAt","updatedAt")
                     VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"#;
    exec(&db, product, &[&beer, "Cerveja Lata", "8", &cat, "240", "1", "1", "12", "84", "BAR", n, n]).await;
    exec(&db, product, &[&caipirinha, "Caipirinha", "18", &cat, "0", "0", "0", "24", "", "BAR", n, n]).await;
    exec(&db, product, &[&cachaca, "Cachaça (dose ml)", "0", &cat, "1000", "1", "0", "24", "", "NONE", n, n]).await;
    exec(&db, product, &[&burger, "Hambúrguer", "32", &cat, "50", "0", "0", "24", "", "KITCHEN", n, n]).await;
    exec(&db, r#"UPDATE "Product" SET "boxPrice" = NULL WHERE "boxPrice" = ''"#, &[]).await;
    exec(&db, r#"INSERT INTO "ProductComponent" ("id","parentProductId","componentId","quantity","createdAt","updatedAt") VALUES (?,?,?,50,?,?)"#,
        &[&id("pc"), &caipirinha, &cachaca, n, n]).await;
    exec(&db, r#"INSERT INTO "ModifierGroup" ("id","name","minSelect","maxSelect","createdAt","updatedAt") VALUES (?, 'Ponto', 1, 2, ?, ?)"#, &[&group, n, n]).await;
    exec(&db, r#"INSERT INTO "ModifierOption" ("id","groupId","name","price") VALUES (?,?, 'Mal passado', 0)"#, &[&rare, &group]).await;
    exec(&db, r#"INSERT INTO "ModifierOption" ("id","groupId","name","price") VALUES (?,?, 'Bacon', 5.5)"#, &[&bacon, &group]).await;
    exec(&db, r#"INSERT INTO "ProductModifierGroup" ("productId","groupId") VALUES (?,?)"#, &[&burger, &group]).await;

    Fixture { db, table1, table2, beer, caipirinha, cachaca, burger, rare, bacon }
}

pub(crate) async fn stock(db: &Db, id: &str) -> f64 {
    sqlx::query_scalar(r#"SELECT CAST("stock" AS REAL) FROM "Product" WHERE "id" = ?"#).bind(id).fetch_one(db.pool()).await.unwrap()
}

pub(crate) fn line(product: &str, qty: i64) -> NewLine {
    NewLine { product_id: product.into(), quantity: qty, modifier_option_ids: Some(vec![]), ..Default::default() }
}

#[tokio::test]
async fn open_add_and_totals() {
    let f = fixture().await;
    let (order, events) = open_table(&f.db, &f.table1, OpenTable { customer_count: 3, ..Default::default() }).await.unwrap();
    assert_eq!(events, vec![Event::table(&f.table1, "opened")]);
    assert!(matches!(open_table(&f.db, &f.table1, OpenTable::default()).await, Err(DbError::Conflict(_))));

    let mut box_line = line(&f.beer, 2);
    box_line.unit_type = Some("BOX".into());
    let burger = NewLine { modifier_option_ids: Some(vec![f.bacon.clone()]), notes: Some(" sem cebola ".into()), ..line(&f.burger, 1) };
    let (created, events) =
        add_items(&f.db, &order, &[line(&f.beer, 3), box_line, line(&f.caipirinha, 2), burger], noon()).await.unwrap();
    assert_eq!(created.len(), 4);
    assert!(matches!(events.last(), Some(Event::KdsNewOrder { item_ids, .. }) if item_ids.len() == 4));

    // 3×8 + 2 caixas×84 + 2×18 + (32+5,5)
    let d = detail(&f.db, &order).await.unwrap();
    assert_eq!(d.head.subtotal, 24.0 + 168.0 + 36.0 + 37.5);
    assert_eq!(d.head.total, d.head.subtotal);
    let box_item = d.items.iter().find(|i| i.unit_type.as_deref() == Some("BOX")).unwrap();
    assert_eq!(box_item.notes.as_deref(), Some("📦 Caixa Fechada (12 un)"));
    let burger_item = d.items.iter().find(|i| i.product_id == f.burger).unwrap();
    assert_eq!(burger_item.notes.as_deref(), Some("sem cebola"));
    assert!(burger_item.modifiers.as_deref().unwrap().contains("Bacon"));

    // Estoque: 240 − 3 − 2×12; ficha técnica 2 × 50 ml
    assert_eq!(stock(&f.db, &f.beer).await, 213.0);
    assert_eq!(stock(&f.db, &f.cachaca).await, 900.0);

    // 10% e desconto
    set_service_fee(&f.db, &order, Some(true), None).await.unwrap();
    let d = detail(&f.db, &order).await.unwrap();
    assert_eq!((d.head.service_fee, d.head.total), (26.55, 292.05));
    set_discount(&f.db, &order, 2.05, Some("Caixa")).await.unwrap();
    assert_eq!(detail(&f.db, &order).await.unwrap().head.total, 290.0);
}

#[tokio::test]
async fn validation_and_cancel_returns_box_stock() {
    let f = fixture().await;
    let (order, _) = open_table(&f.db, &f.table1, OpenTable::default()).await.unwrap();

    let err = add_items(&f.db, &order, &[line(&f.burger, 1)], noon()).await.unwrap_err();
    assert_eq!(err.to_string(), "Escolha \"Ponto\" para Hambúrguer.");
    let err = add_items(&f.db, &order, &[line("sumiu", 1)], noon()).await.unwrap_err();
    assert!(err.to_string().contains("não existe mais"));
    // Nada foi gravado nas tentativas recusadas
    assert!(detail(&f.db, &order).await.unwrap().items.is_empty());

    let mut box_line = line(&f.beer, 1);
    box_line.unit_type = Some("BOX".into());
    let (created, _) = add_items(&f.db, &order, &[box_line], noon()).await.unwrap();
    assert_eq!(stock(&f.db, &f.beer).await, 228.0);
    remove_item(&f.db, &order, &created[0], Some("Gerente")).await.unwrap();
    assert_eq!(stock(&f.db, &f.beer).await, 240.0, "caixa cancelada devolve as 12 unidades");
    assert_eq!(detail(&f.db, &order).await.unwrap().head.total, 0.0);

    let audit: (String, Option<String>) =
        sqlx::query_as(r#"SELECT "description", "userId" FROM "AuditLog" WHERE "action" = 'CANCEL_ITEM'"#)
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(audit.0, "Cancelado 1x Cerveja Lata (Comanda #101 - Mesa 1)");
    assert_eq!(audit.1.as_deref(), Some("Gerente"));
}

#[tokio::test]
async fn haver_blocks_reducing_total_below_paid() {
    let f = fixture().await;
    let (order, _) = open_table(&f.db, &f.table1, OpenTable::default()).await.unwrap();
    let (created, _) = add_items(&f.db, &order, &[line(&f.beer, 5)], noon()).await.unwrap();
    sqlx::query(r#"UPDATE "Order" SET "paidAmount" = 30 WHERE "id" = ?"#).bind(&order).execute(f.db.pool()).await.unwrap();
    assert!(matches!(remove_item(&f.db, &order, &created[0], None).await, Err(DbError::Rule(_))));
    assert!(matches!(set_discount(&f.db, &order, 15.0, None).await, Err(DbError::Rule(_))));
    set_discount(&f.db, &order, 10.0, None).await.unwrap();
}

#[tokio::test]
async fn transfer_and_merge() {
    let f = fixture().await;
    let (order, _) = open_table(&f.db, &f.table1, OpenTable { customer_name: Some("Ana".into()), customer_count: 2, ..Default::default() }).await.unwrap();
    add_items(&f.db, &order, &[line(&f.beer, 2)], noon()).await.unwrap();

    let (msg, _) = transfer_table(&f.db, &f.table1, &f.table2).await.unwrap();
    assert_eq!(msg, "Mesa 1 transferida para Mesa 2");
    let cards = tables::list_cards(&f.db).await.unwrap();
    assert_eq!((cards[0].status.as_str(), cards[0].order_id.is_none()), ("AVAILABLE", true));
    assert_eq!(cards[1].order_id.as_deref(), Some(order.as_str()));
    assert_eq!(cards[1].customer_name.as_deref(), Some("Ana"));
    assert!(transfer_table(&f.db, &f.table1, &f.table2).await.is_err());

    // Mesa 1 reabre com haver; junta na mesa 2
    let (order1, _) = open_table(&f.db, &f.table1, OpenTable::default()).await.unwrap();
    add_items(&f.db, &order1, &[line(&f.beer, 1)], noon()).await.unwrap();
    sqlx::query(r#"UPDATE "Order" SET "paidAmount" = 8 WHERE "id" = ?"#).bind(&order1).execute(f.db.pool()).await.unwrap();
    let (msg, _) = merge_tables(&f.db, &f.table2, &f.table1).await.unwrap();
    assert_eq!(msg, "Mesa 1 unida à Mesa 2");
    let d = detail(&f.db, &order).await.unwrap();
    assert_eq!((d.items.len(), d.head.total, d.head.paid_amount), (2, 24.0, 8.0));
    let cancelled: String = sqlx::query_scalar(r#"SELECT "status" FROM "Order" WHERE "id" = ?"#).bind(&order1).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(cancelled, "CANCELLED");
}

#[tokio::test]
async fn closing_and_new_items_reopen() {
    let f = fixture().await;
    let (order, _) = open_table(&f.db, &f.table1, OpenTable::default()).await.unwrap();
    request_closing(&f.db, &f.table1).await.unwrap();
    assert_eq!(tables::list_cards(&f.db).await.unwrap()[0].status, "CLOSING");
    add_items(&f.db, &order, &[line(&f.beer, 1)], noon()).await.unwrap();
    assert_eq!(tables::list_cards(&f.db).await.unwrap()[0].status, "OCCUPIED");
    let _ = (&f.rare, &f.cachaca);
}

#[tokio::test]
async fn payment_flow_cash_haver_and_fiado() {
    let f = fixture().await;
    let (order, _) = open_table(&f.db, &f.table1, OpenTable::default()).await.unwrap();
    let (items, _) = add_items(&f.db, &order, &[line(&f.beer, 5)], noon()).await.unwrap(); // 5 x 8 = 40.0

    // 1. Tentar pagar sem caixa aberto -> erro Forbidden
    let pay_input = ProcessPaymentInput {
        payments: vec![NewPayment {
            amount: 20.0,
            method: "CASH".into(),
            notes: None,
        }],
        close_order: None,
        customer_id: None,
        paid_items: None,
    };
    assert!(matches!(process_payment(&f.db, &order, pay_input.clone()).await, Err(DbError::Forbidden(_))));

    // 2. Abre o caixa
    crate::cash::open_shift(&f.db, crate::cash::OpenShiftInput { initial_balance: 100.0, ..Default::default() }).await.unwrap();

    // 3. Tentar pagar mais do que o total -> erro Rule
    let overpay = ProcessPaymentInput {
        payments: vec![NewPayment {
            amount: 50.0,
            method: "CASH".into(),
            notes: None,
        }],
        close_order: None,
        customer_id: None,
        paid_items: None,
    };
    assert!(matches!(process_payment(&f.db, &order, overpay).await, Err(DbError::Rule(_))));

    // 4. Abatimento parcial (haver) R$ 15,00 em dinheiro + racha de 2 itens
    let haver = ProcessPaymentInput {
        payments: vec![NewPayment {
            amount: 15.0,
            method: "CASH".into(),
            notes: Some("Haver parcial".into()),
        }],
        close_order: None,
        customer_id: None,
        paid_items: Some(vec![PaidItemLine { item_id: items[0].clone(), quantity: 2 }]),
    };
    let (res, events) = process_payment(&f.db, &order, haver).await.unwrap();
    assert_eq!(res.paid_amount, 15.0);
    assert!(!res.is_fully_paid);
    assert_eq!(res.status, "OPEN");
    assert!(events.contains(&Event::order(&order, "paid")));
    assert!(events.contains(&Event::CashUpdated));

    // A mesa continua ocupada
    let cards = tables::list_cards(&f.db).await.unwrap();
    assert_eq!(cards[0].status, "OCCUPIED");
    assert_eq!(cards[0].paid_amount, Some(15.0));

    // Conferir item paidQuantity
    let paid_qty: i64 = sqlx::query_scalar(r#"SELECT "paidQuantity" FROM "OrderItem" WHERE "id" = ?"#)
        .bind(&items[0])
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(paid_qty, 2);

    // 5. Cadastrar cliente e pagar o restante (R$ 25,00) via FIADO (CREDIT_TAB), fechando a conta
    let cust_id = barerp_core::ids::cuid();
    sqlx::query(r#"INSERT INTO "Customer" ("id", "name", "creditTabBalance", "createdAt", "updatedAt") VALUES (?, 'João Fiado', 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"#)
        .bind(&cust_id)
        .execute(f.db.pool())
        .await
        .unwrap();

    let pay_rest = ProcessPaymentInput {
        payments: vec![NewPayment {
            amount: 25.0,
            method: "CREDIT_TAB".into(),
            notes: Some("Pendura pro João".into()),
        }],
        close_order: Some(true),
        customer_id: Some(cust_id.clone()),
        paid_items: None,
    };
    let (res2, events2) = process_payment(&f.db, &order, pay_rest).await.unwrap();
    assert_eq!(res2.paid_amount, 40.0);
    assert!(res2.is_fully_paid);
    assert_eq!(res2.status, "PAID");
    assert!(events2.contains(&Event::table(&f.table1, "closed")));

    // Saldo do fiado do cliente subiu para 25.0
    let balance: f64 = sqlx::query_scalar(r#"SELECT "creditTabBalance" FROM "Customer" WHERE "id" = ?"#)
        .bind(&cust_id)
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(balance, 25.0);

    // Mesa foi liberada
    let cards = tables::list_cards(&f.db).await.unwrap();
    assert_eq!(cards[0].status, "AVAILABLE");
    assert!(cards[0].order_id.is_none());

    // Não pode pagar comanda já paga/fechada (Conflict)
    let re_pay = ProcessPaymentInput {
        payments: vec![NewPayment { amount: 10.0, method: "CASH".into(), notes: None }],
        close_order: None,
        customer_id: None,
        paid_items: None,
    };
    assert!(matches!(process_payment(&f.db, &order, re_pay).await, Err(DbError::Conflict(_))));
}

#[tokio::test]
async fn quick_sale_flow() {
    let f = fixture().await;
    let now = chrono::Local::now().naive_local();

    // 1. Tentar venda rápida sem caixa aberto -> erro
    let input = QuickSaleInput {
        items: vec![NewLine {
            product_id: f.beer.clone(),
            quantity: 2,
            notes: None,
            unit_type: None,
            modifier_option_ids: None,
        }],
        payment: QuickSalePayment {
            method: "CASH".into(),
            amount: Some(15.0),
            cash_tendered: Some(20.0),
            notes: None,
        },
        customer_name: Some("Cliente Balcão 1".into()),
        discount: Some(1.0), // 16.00 - 1.00 = 15.00
        expected_total: Some(15.0),
    };
    assert!(matches!(quick_sale(&f.db, input.clone(), now).await, Err(DbError::Forbidden(_))));

    // 2. Abrir caixa
    crate::cash::open_shift(&f.db, crate::cash::OpenShiftInput { initial_balance: 50.0, ..Default::default() }).await.unwrap();

    // 3. Concluir venda rápida com troco
    let (res, events) = quick_sale(&f.db, input, now).await.unwrap();
    assert_eq!(res.order.head.status, "PAID");
    assert_eq!(res.order.head.table_id, None);
    assert_eq!(res.order.head.customer_name.as_deref(), Some("Cliente Balcão 1"));
    assert_eq!(res.order.head.subtotal, 16.0);
    assert_eq!(res.order.head.discount, 1.0);
    assert_eq!(res.order.head.total, 15.0);
    assert_eq!(res.order.head.paid_amount, 15.0);
    assert_eq!(res.change, 5.0);
    assert_eq!(res.order.payments.len(), 1);
    assert_eq!(res.order.payments[0].method, "CASH");
    assert_eq!(res.order.payments[0].notes.as_deref(), Some("Troco: R$ 5.00 (Recebido: R$ 20.00)"));
    assert!(events.contains(&Event::CashUpdated));

    // 4. Divergência de total calculado (mudança de promoção) -> Conflict
    let conflict_input = QuickSaleInput {
        items: vec![NewLine {
            product_id: f.beer.clone(),
            quantity: 1,
            notes: None,
            unit_type: None,
            modifier_option_ids: None,
        }],
        payment: QuickSalePayment {
            method: "PIX".into(),
            amount: None,
            cash_tendered: None,
            notes: None,
        },
        customer_name: None,
        discount: None,
        expected_total: Some(5.0), // Total real é 8.00
    };
    assert!(matches!(quick_sale(&f.db, conflict_input, now).await, Err(DbError::Conflict(_))));
}



#[tokio::test]
async fn payment_guards_fiado_and_foreign_items() {
    let f = fixture().await;
    crate::cash::open_shift(&f.db, crate::cash::OpenShiftInput { initial_balance: 0.0, ..Default::default() }).await.unwrap();
    let (order1, _) = open_table(&f.db, &f.table1, OpenTable::default()).await.unwrap();
    let (order2, _) = open_table(&f.db, &f.table2, OpenTable::default()).await.unwrap();
    add_items(&f.db, &order1, &[line(&f.beer, 2)], noon()).await.unwrap();
    let (items2, _) = add_items(&f.db, &order2, &[line(&f.beer, 3)], noon()).await.unwrap();

    let pay = |method: &str, customer: Option<String>, paid_items: Option<Vec<PaidItemLine>>| ProcessPaymentInput {
        payments: vec![NewPayment { amount: 8.0, method: method.into(), notes: None }],
        close_order: Some(false),
        customer_id: customer,
        paid_items,
    };

    // Fiado sem cliente é recusado e nada é gravado
    let err = process_payment(&f.db, &order1, pay("CREDIT_TAB", None, None)).await.unwrap_err();
    assert_eq!(err.to_string(), "Escolha o cliente para lançar no fiado.");
    assert_eq!(detail(&f.db, &order1).await.unwrap().head.paid_amount, 0.0);

    // Item de outra comanda não pode ser marcado como pago nesta
    let foreign = Some(vec![PaidItemLine { item_id: items2[0].clone(), quantity: 1 }]);
    assert!(matches!(process_payment(&f.db, &order1, pay("CASH", None, foreign)).await, Err(DbError::Conflict(_))));
    assert_eq!(detail(&f.db, &order1).await.unwrap().head.paid_amount, 0.0, "a transação foi desfeita");

    // Pagas nunca passam da quantidade do item
    let too_many = Some(vec![PaidItemLine { item_id: items2[0].clone(), quantity: 10 }]);
    process_payment(&f.db, &order2, pay("PIX", None, too_many)).await.unwrap();
    let paid: i64 = sqlx::query_scalar(r#"SELECT "paidQuantity" FROM "OrderItem" WHERE "id" = ?"#)
        .bind(&items2[0])
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(paid, 3);
}

#[tokio::test]
async fn quick_sale_payment_is_the_server_total() {
    let f = fixture().await;
    crate::cash::open_shift(&f.db, crate::cash::OpenShiftInput { initial_balance: 0.0, ..Default::default() }).await.unwrap();
    let sale = |amount: Option<f64>| QuickSaleInput {
        items: vec![line(&f.beer, 2)],
        payment: QuickSalePayment { method: "PIX".into(), amount, cash_tendered: None, notes: None },
        customer_name: None,
        discount: None,
        expected_total: None,
    };
    // App antigo mandando um valor diferente do total (16,00) é recusado, como no Node
    assert!(matches!(quick_sale(&f.db, sale(Some(10.0)), noon()).await, Err(DbError::Conflict(_))));
    // Sem valor informado, o pagamento gravado é o total calculado
    let (res, _) = quick_sale(&f.db, sale(None), noon()).await.unwrap();
    assert_eq!((res.order.head.total, res.order.payments[0].amount), (16.0, 16.0));
}
