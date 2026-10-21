use super::*;

async fn exec(db: &Db, sql: &str, binds: &[&str]) {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(db.pool()).await.unwrap();
}

async fn setup() -> (Db, String, String) {
    let db = Db::open_in_memory().await.unwrap();
    let (bebidas, cozinha) = (ids::cuid(), ids::cuid());
    exec(&db, r#"INSERT INTO "Category" ("id","name","sortOrder","codeStart") VALUES (?, 'Bebidas', 0, 5001)"#, &[&bebidas]).await;
    exec(&db, r#"INSERT INTO "Category" ("id","name","sortOrder","codeStart") VALUES (?, 'Cozinha', 1, 1001)"#, &[&cozinha]).await;
    (db, bebidas, cozinha)
}

fn input(name: &str, category: &str, price: f64) -> ProductInput {
    ProductInput {
        name: name.into(),
        category_id: category.into(),
        price,
        kds_station: "BAR".into(),
        unit: "un".into(),
        stock: 10.0,
        min_stock: 2.0,
        show_in_menu: true,
        ..Default::default()
    }
}

async fn sell(db: &Db, product: &str, status: &str) {
    let order = ids::cuid();
    let n = DbTime::now().0.timestamp_millis().to_string();
    exec(db, r#"INSERT INTO "Order" ("id","status","createdAt","updatedAt") VALUES (?,?,?,?)"#, &[&order, status, &n, &n]).await;
    exec(
        db,
        r#"INSERT INTO "OrderItem" ("id","orderId","productId","quantity","unitPrice","totalPrice","updatedAt") VALUES (?,?,?,1,10,10,?)"#,
        &[&ids::cuid(), &order, product, &n],
    )
    .await;
}

#[tokio::test]
async fn cria_com_codigo_da_faixa_historico_e_ficha() {
    let (db, bebidas, _) = setup().await;
    let (cachaca, ev) = create(&db, input("Cachaça", &bebidas, 5.0), Some("Ana")).await.unwrap();
    assert_eq!(ev, vec![Event::CatalogUpdated { scope: "products" }]);
    let (limao, _) = create(&db, input("Limão", &bebidas, 1.0), None).await.unwrap();

    let mut caipi = input("Caipirinha", &bebidas, 18.0);
    caipi.cost_price = Some(6.0);
    caipi.code = Some("5001".into()); // já usado: ganha o próximo
    caipi.components = vec![(cachaca.clone(), 0.05), (limao.clone(), 1.0), (cachaca.clone(), 0.05), (limao.clone(), 0.0)];
    let (id, _) = create(&db, caipi, Some("Ana")).await.unwrap();

    let d = detail(&db, &id).await.unwrap();
    assert_eq!(d.product.code.as_deref(), Some("5003"));
    assert_eq!(d.product.component_count, 2);
    assert_eq!(d.components.iter().map(|c| (c.name.as_str(), c.quantity)).collect::<Vec<_>>(), vec![("Cachaça", 0.1), ("Limão", 1.0)]);
    let h = price_history(&db, &id).await.unwrap();
    assert_eq!((h.len(), h[0].reason.as_str(), h[0].changed_by.as_str()), (1, "CADASTRO_INICIAL", "Ana"));

    assert!(matches!(create(&db, input(" ", &bebidas, 1.0), None).await, Err(DbError::Rule(_))));
    assert!(matches!(create(&db, input("X", &bebidas, -1.0), None).await, Err(DbError::Rule(_))));
}

#[tokio::test]
async fn altera_com_historico_e_codigo_unico() {
    let (db, bebidas, _) = setup().await;
    let (a, _) = create(&db, input("Chopp", &bebidas, 10.0), None).await.unwrap();
    let (b, _) = create(&db, input("Água", &bebidas, 4.0), None).await.unwrap();

    let mut change = input("Chopp 500ml", &bebidas, 12.0);
    change.cost_price = Some(4.0);
    change.code = Some("5001".into());
    update(&db, &a, change.clone(), Some("Bia")).await.unwrap();
    let h = price_history(&db, &a).await.unwrap();
    assert_eq!(h[0].reason, "MANUAL_UPDATE");
    assert_eq!((h[0].old_price, h[0].new_price, h[0].price_diff, h[0].price_percent), (10.0, 12.0, 2.0, 20.0));
    assert_eq!(h[0].cost_diff, Some(4.0));

    // Sem mudar preço/custo não grava histórico
    update(&db, &a, change, None).await.unwrap();
    assert_eq!(price_history(&db, &a).await.unwrap().len(), 2);

    let mut dup = input("Água", &bebidas, 4.0);
    dup.code = Some("5001".into());
    let e = update(&db, &b, dup, None).await.unwrap_err();
    assert!(matches!(e, DbError::Rule(m) if m.contains("já está em uso pelo produto \"Chopp 500ml\"")));
    assert!(matches!(update(&db, "x", input("X", &bebidas, 1.0), None).await, Err(DbError::NotFound(_))));
}

#[tokio::test]
async fn fornecedor_avulso_liga_ao_cadastrado() {
    let (db, bebidas, _) = setup().await;
    exec(&db, r#"INSERT INTO "Supplier" ("id","name","tradeName","updatedAt") VALUES ('s1','Ambev S.A.','Ambev',0)"#, &[]).await;
    let mut p = input("Brahma", &bebidas, 8.0);
    p.supplier = Some("ambev".into());
    let (id, _) = create(&db, p, None).await.unwrap();
    let r = detail(&db, &id).await.unwrap().product;
    assert_eq!((r.supplier_id.as_deref(), r.supplier_label()), (Some("s1"), Some("Ambev")));

    let mut q = input("Skol", &bebidas, 8.0);
    q.supplier_id = Some("s1".into());
    let (id, _) = create(&db, q, None).await.unwrap();
    assert_eq!(detail(&db, &id).await.unwrap().product.supplier.as_deref(), Some("Ambev"));
}

#[tokio::test]
async fn excluir_preserva_historico_de_venda() {
    let (db, bebidas, cozinha) = setup().await;
    let (novo, _) = create(&db, input("Nunca vendido", &bebidas, 5.0), None).await.unwrap();
    let (vendido, _) = create(&db, input("Já vendido", &bebidas, 5.0), None).await.unwrap();
    let (aberto, _) = create(&db, input("Na mesa", &bebidas, 5.0), None).await.unwrap();
    sell(&db, &vendido, "PAID").await;
    sell(&db, &aberto, "OPEN").await;

    delete(&db, &novo).await.unwrap();
    assert!(matches!(detail(&db, &novo).await, Err(DbError::NotFound(_))));

    delete(&db, &vendido).await.unwrap();
    let r = detail(&db, &vendido).await.unwrap().product;
    assert!(!r.is_active && !r.show_in_menu, "só desativado");
    let items: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "OrderItem" WHERE "productId" = ?"#).bind(&vendido).fetch_one(db.pool()).await.unwrap();
    assert_eq!(items, 1, "comanda antiga intacta");
    assert!(!list(&db).await.unwrap().iter().any(|p| p.id == vendido));

    let e = delete(&db, &aberto).await.unwrap_err();
    assert!(matches!(e, DbError::Rule(m) if m.contains("comanda(s) aberta(s)")));

    // Insumo de ficha técnica não sai
    let (insumo, _) = create(&db, input("Batata", &cozinha, 1.0), None).await.unwrap();
    let mut fritas = input("Fritas", &cozinha, 20.0);
    fritas.components = vec![(insumo.clone(), 0.3)];
    create(&db, fritas, None).await.unwrap();
    let e = delete(&db, &insumo).await.unwrap_err();
    assert!(matches!(e, DbError::Rule(m) if m.contains("ficha técnica de Fritas")));
}

#[tokio::test]
async fn categorias_faixas_e_exclusao() {
    let (db, bebidas, cozinha) = setup().await;
    let cats = categories(&db).await.unwrap();
    assert_eq!(suggest_code_start(&cats), 2001);
    let (pizza, _) = create_category(&db, "Pizzas", None, None).await.unwrap();
    let (doces, _) = create_category(&db, "Doces", None, None).await.unwrap();
    let (outra, _) = create_category(&db, "Vinhos", None, None).await.unwrap(); // 5001 ocupada → próxima livre
    let cats = categories(&db).await.unwrap();
    let start = |id: &str| cats.iter().find(|c| c.id == id).unwrap().code_start;
    assert_eq!((start(&pizza), start(&doces), start(&outra)), (Some(3001), Some(6001), Some(2001)));
    assert!(matches!(create_category(&db, "  ", None, None).await, Err(DbError::Rule(_))));

    assert_eq!(next_code(&db, &cozinha).await.unwrap(), "1001");
    let (vendido, _) = create(&db, input("X-Burguer", &cozinha, 30.0), None).await.unwrap();
    create(&db, input("X-Salada", &cozinha, 32.0), None).await.unwrap();
    sell(&db, &vendido, "PAID").await;
    let e = delete_category(&db, &cozinha).await.unwrap_err();
    assert!(matches!(e, DbError::Rule(m) if m.contains("1 produto(s) que já foram vendidos")));

    create(&db, input("Suco", &bebidas, 7.0), None).await.unwrap();
    delete_category(&db, &bebidas).await.unwrap();
    assert!(!categories(&db).await.unwrap().iter().any(|c| c.id == bebidas));

    // Renumerar ao editar
    update_category(&db, &cozinha, "Cozinha", Some("ChefHat"), Some(1101), false).await.unwrap();
    let (msg, _) = update_category(&db, &cozinha, "Cozinha", None, None, true).await.unwrap();
    assert!(msg.unwrap().contains("2 produtos da categoria \"Cozinha\" renumerados sequencialmente a partir de #1101"));
    let codes: Vec<String> = list(&db).await.unwrap().into_iter().filter_map(|p| p.code).collect();
    assert_eq!(codes, vec!["1101", "1102"]);
}

#[tokio::test]
async fn codigos_gerar_e_corrigir_duplicados() {
    let (db, bebidas, cozinha) = setup().await;
    let n = DbTime::now().0.timestamp_millis().to_string();
    for (name, code, cat) in [("A", "5001", &bebidas), ("B", "5001", &bebidas), ("C", "", &cozinha), ("D", "5001", &cozinha)] {
        exec(
            &db,
            r#"INSERT INTO "Product" ("id","name","code","price","categoryId","updatedAt") VALUES (?,?,NULLIF(?, ''),1,?,?)"#,
            &[&ids::cuid(), name, code, cat, &n],
        )
        .await;
    }
    assert_eq!(duplicate_code_count(&list(&db).await.unwrap()), 3);
    let (msg, _) = backfill_codes(&db).await.unwrap();
    assert!(msg.contains("1 produtos receberam"));
    let (msg, _) = fix_duplicate_codes(&db, false).await.unwrap();
    assert!(msg.contains("2 produto(s)"), "{msg}");
    let products = list(&db).await.unwrap();
    assert_eq!(duplicate_code_count(&products), 0);
    let (msg, ev) = fix_duplicate_codes(&db, false).await.unwrap();
    assert!(msg.contains("100% íntegro") && ev.is_empty());

    let (msg, _) = fix_duplicate_codes(&db, true).await.unwrap();
    assert!(msg.contains("(4 produtos)"));
    let mut codes: Vec<String> = list(&db).await.unwrap().into_iter().filter_map(|p| p.code).collect();
    codes.sort();
    assert_eq!(codes, vec!["1001", "1002", "5001", "5002"]);
}

#[tokio::test]
async fn estoque_relatorio_e_organizar() {
    let (db, bebidas, cozinha) = setup().await;
    let mut chopp = input("Chopp Pilsen 300ml", &cozinha, 10.0);
    chopp.cost_price = Some(4.0);
    chopp.kds_station = "KITCHEN".into();
    let (chopp_id, _) = create(&db, chopp, None).await.unwrap();
    let mut x = input("X-Burguer", &cozinha, 30.0);
    x.cost_price = Some(12.0);
    x.stock = -3.0;
    create(&db, x, None).await.unwrap();

    assert_eq!(adjust_stock(&db, &chopp_id, 10).await.unwrap().0, 20);
    assert_eq!(adjust_stock(&db, &chopp_id, -1).await.unwrap().0, 19);
    assert!(matches!(adjust_stock(&db, "x", 1).await, Err(DbError::NotFound(_))));

    let r = stock_report(&db).await.unwrap();
    assert_eq!((r.total_products, r.total_stock_units), (2, 19), "estoque negativo conta 0");
    assert_eq!((r.total_cost_value, r.total_sale_value, r.total_expected_profit), (76.0, 190.0, 114.0));
    assert_eq!(r.average_margin, 60.0);
    let coz = r.categories.iter().find(|c| c.category == "Cozinha").unwrap();
    assert_eq!((coz.items_count, coz.units_count, coz.margin_percent), (2, 19, 60.0));

    let (msg, _) = organize_bar_kitchen(&db).await.unwrap();
    assert!(msg.contains("1 produtos no Bar"), "{msg}");
    let p = detail(&db, &chopp_id).await.unwrap().product;
    assert_eq!(p.kds_station, "BAR");
    assert_eq!(p.category_name.as_deref(), Some("Bar"), "categoria Bar criada");
    let _ = bebidas;
}

#[test]
fn classifica_bebida_como_o_node() {
    assert!(is_bar_product("Heineken Long Neck 330ml", None, None));
    assert!(is_bar_product("Coca 2 L", None, None));
    assert!(is_bar_product("Guaraná lata", None, None));
    assert!(is_bar_product("Gin tônica", None, None));
    assert!(is_bar_product("Qualquer", None, Some("lt")));
    assert!(!is_bar_product("Original", None, None), "gin seguido de letra não conta");
    // Como no Node: o `\b` do JS só conhece letras ASCII, então "gin" antes de "á" conta como palavra
    assert!(is_bar_product("Ginástica", None, None));
    assert!(!is_bar_product("X-Burguer 150g", None, None));
    assert!(!is_bar_product("Mesa 12", Some("lugares"), None));
    assert!(has_volume("Chopp 500 ml") && has_volume("Água 1,5l") && !has_volume("Picanha 1kg") && !has_volume("x350ml"));
}

#[test]
fn margem_e_estoque_baixo() {
    let mut r = ProductRow {
        id: "1".into(),
        code: None,
        ean: None,
        name: "A".into(),
        description: None,
        brand: None,
        supplier: None,
        supplier_id: None,
        supplier_name: None,
        category_id: "c".into(),
        category_name: None,
        kds_station: "BAR".into(),
        price: 10.0,
        cost_price: Some(4.0),
        target_margin: None,
        stock: 2,
        min_stock: 2,
        track_stock: true,
        unit: "un".into(),
        has_box_price: false,
        box_quantity: None,
        box_price: None,
        box_cost_price: None,
        box_ean: None,
        ncm: None,
        cfop: None,
        cest: None,
        is_active: true,
        show_in_menu: true,
        image_hash: None,
        component_count: 0,
    };
    assert_eq!(r.margin_percent(), Some(60));
    assert!(r.is_low_stock());
    r.cost_price = None;
    assert_eq!(r.margin_percent(), None);
}
