//! Sobe só o servidor dos celulares, para testar os APKs sem abrir a tela do caixa.
//! `cargo run -p barerp-server --example serve -- <cópia do bar.db> [porta]`

use std::path::PathBuf;

use barerp_db::Db;
use barerp_server::{Server, DEFAULT_PORT};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("informe o caminho de uma cópia do bar.db")?);
    let port = args.next().map(|p| p.parse()).transpose()?.unwrap_or(DEFAULT_PORT);
    let db = Db::open(&path).await?;
    let data_dir = path.parent().map(PathBuf::from).unwrap_or_default();
    Server::new(db, data_dir, port).serve().await?;
    Ok(())
}
