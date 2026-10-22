//! Banco de demonstração para os prints das telas (`mesas_shots`, `cardapio_shots`) sem a cópia do cliente:
//! `cargo run -p barerp-db --example demo_db -- <arquivo novo>`.
//! Cria mesas em três setores, um cardápio de bar e comandas abertas com tempos variados.

use std::path::PathBuf;

use barerp_core::ids;
use barerp_db::orders::{self, NewLine, OpenTable};
use barerp_db::table_admin::{self, TableInput};
use barerp_db::{Db, DbTime};

async fn exec(db: &Db, sql: &str, binds: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(db.pool()).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::args().nth(1).ok_or("informe o arquivo do banco")?);
    if path.exists() {
        return Err("o arquivo já existe; informe um caminho novo".into());
    }
    let db = Db::open(&path).await?;
    let now = DbTime::now().0;
    let n = now.timestamp_millis().to_string();

    let mut categories = Vec::new();
    for (i, name) in ["Cervejas", "Drinks", "Porções", "Sem álcool"].iter().enumerate() {
        let id = ids::cuid();
        exec(&db, r#"INSERT INTO "Category" ("id","name","sortOrder","codeStart") VALUES (?,?,?,?)"#,
            &[&id, name, &i.to_string(), &(1001 + i * 1000).to_string()]).await?;
        categories.push(id);
    }
    let menu = [
        ("Heineken Long Neck", "12", 0, "BAR", "7891000000011"),
        ("Chopp Brahma 300ml", "9.9", 0, "BAR", ""),
        ("Original 600ml", "16", 0, "BAR", "7891000000028"),
        ("Caipirinha de Limão", "22", 1, "BAR", ""),
        ("Gin Tônica", "28", 1, "BAR", ""),
        ("Dose de Cachaça", "8", 1, "BAR", ""),
        ("Porção de Fritas", "35", 2, "KITCHEN", ""),
        ("Isca de Peixe", "59", 2, "KITCHEN", ""),
        ("Pastel de Carne", "9", 2, "KITCHEN", ""),
        ("Picanha na Chapa", "89", 2, "KITCHEN", ""),
        ("Frango à Passarinho", "48", 2, "KITCHEN", ""),
        ("Coca-Cola 350ml", "7", 3, "BAR", "7894900011517"),
        ("Água Mineral 500ml", "5", 3, "BAR", "7896000000035"),
        ("Red Bull", "15", 3, "BAR", ""),
    ];
    let mut products = Vec::new();
    for (i, (name, price, cat, station, ean)) in menu.iter().enumerate() {
        let id = ids::cuid();
        let code = (1001 + cat * 1000 + i).to_string();
        exec(&db, r#"INSERT INTO "Product" ("id","code","ean","name","price","categoryId","kdsStation","stock","createdAt","updatedAt")
                     VALUES (?,?,?,?,?,?,?,200,?,?)"#,
            &[&id, &code, ean, name, price, &categories[*cat], station, &n, &n]).await?;
        products.push(id);
    }
    exec(&db, r#"UPDATE "Product" SET "ean" = NULL WHERE "ean" = ''"#, &[]).await?;

    let sections = [("Salão", 1..=7, 4), ("Varanda", 8..=12, 6), ("Balcão", 13..=14, 1)];
    let mut tables = Vec::new();
    for (section, numbers, capacity) in sections {
        for number in numbers {
            let input = TableInput { number, name: None, capacity, section: Some(section.into()) };
            let (id, _) = table_admin::create_table(&db, input).await?;
            tables.push(id);
        }
    }

    // (mesa, cliente, pessoas, minutos desde a abertura, minutos desde o último pedido, itens)
    let open: [(usize, &str, i64, i64, i64, &[(usize, i64, &str)]); 7] = [
        (1, "Mariana", 4, 112, 42, &[(0, 6, ""), (6, 1, "Sem sal"), (3, 2, "")]),
        (3, "Família Souza", 6, 48, 6, &[(9, 1, "Ponto: mal passada"), (1, 8, ""), (11, 3, "")]),
        (4, "Carlos", 2, 70, 25, &[(2, 3, ""), (8, 4, "")]),
        (6, "", 3, 22, 9, &[(1, 4, ""), (7, 1, "")]),
        (8, "Pedro", 2, 65, 35, &[(0, 3, ""), (12, 2, "")]),
        (9, "Ana", 5, 91, 3, &[(4, 2, ""), (10, 1, "Bem crocante"), (0, 8, "")]),
        (11, "Lucas", 4, 36, 1, &[(6, 2, ""), (3, 1, "Com gelo e limão")]),
    ];
    for (table, customer, people, opened, last, items) in open {
        let input = OpenTable {
            customer_name: (!customer.is_empty()).then(|| customer.to_string()),
            waiter_name: Some("João".into()),
            customer_count: people,
            ..Default::default()
        };
        let (order, _) = orders::open_table(&db, &tables[table], input).await?;
        let lines: Vec<NewLine> = items
            .iter()
            .map(|(p, qty, notes)| NewLine {
                product_id: products[*p].clone(),
                quantity: *qty,
                notes: (!notes.is_empty()).then(|| notes.to_string()),
                modifier_option_ids: Some(vec![]),
                ..Default::default()
            })
            .collect();
        orders::add_items(&db, &order, &lines, chrono::Local::now().naive_local()).await?;
        let opened_ms = (now - chrono::Duration::minutes(opened)).timestamp_millis().to_string();
        let last_ms = (now - chrono::Duration::minutes(last)).timestamp_millis().to_string();
        exec(&db, r#"UPDATE "Order" SET "createdAt" = ? WHERE "id" = ?"#, &[&opened_ms, &order]).await?;
        exec(&db, r#"UPDATE "Table" SET "openedAt" = ? WHERE "id" = ?"#, &[&opened_ms, &tables[table]]).await?;
        exec(&db, r#"UPDATE "OrderItem" SET "addedAt" = ? WHERE "orderId" = ?"#, &[&last_ms, &order]).await?;
    }
    orders::request_closing(&db, &tables[4]).await?;
    println!("{}", path.display());
    Ok(())
}
