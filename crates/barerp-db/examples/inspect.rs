//! Abre um bar.db existente e mostra funcionários e mapa de mesas: `cargo run --example inspect -- <caminho>`.
//! Aplica as migrações no arquivo — use sempre uma cópia do banco do cliente.

use barerp_db::{staff, tables, Db};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("informe o caminho do banco")?;
    let db = Db::open(path.as_ref()).await?;
    for s in staff::public_list(&db).await? {
        println!("funcionário: {} ({}) {}", s.name, s.role, s.id);
    }
    for c in tables::list_cards(&db).await? {
        println!(
            "mesa {:>3} {:<10} {:<16} itens={} total={:?} aberta={:?}",
            c.number, c.status, c.section, c.item_count, c.total, c.opened_at.map(|t| t.0.to_rfc3339())
        );
    }
    Ok(())
}
