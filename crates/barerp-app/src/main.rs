// Sem janela de console no Windows (o app é aberto pelo atalho, como qualquer programa)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! BarERP Pro nativo: telas em Slint, banco SQLite local (o mesmo `bar.db` da versão Electron).

use std::path::PathBuf;

use slint::ComponentHandle;
use tokio::runtime::Runtime;

use barerp_app::{controller, AppWindow};
use barerp_db::Db;

/// Onde fica o banco. A versão Electron grava no `userData` do app em `%APPDATA%`; procuramos os nomes
/// possíveis da pasta para abrir o banco do cliente. `BARERP_DB` força outro caminho (testes).
fn db_path() -> PathBuf {
    if let Some(path) = std::env::var_os("BARERP_DB") {
        return PathBuf::from(path);
    }
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    for folder in ["BarERP Pro", "bar-erp", "BarERP"] {
        let candidate = base.join(folder).join("bar.db");
        if candidate.exists() {
            return candidate;
        }
    }
    base.join("BarERP Pro").join("bar.db")
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Sem console no Windows: um erro ao abrir (banco travado, pasta sem permissão) precisa aparecer numa janela,
    // senão o app simplesmente não abre
    if let Err(e) = run() {
        tracing::error!("o BarERP não conseguiu abrir: {e}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("BarERP Pro")
            .set_description(format!("O BarERP não conseguiu abrir.\n\n{e}\n\nBanco de dados: {}", db_path().display()))
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let rt = Runtime::new()?;
    let path = db_path();
    let data_dir = path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir)?;
    }
    tracing::info!("banco de dados: {}", path.display());
    let db = rt.block_on(Db::open(&path)).map_err(|e| format!("Erro ao abrir o banco de dados: {e}"))?;

    // Impressora que o Electron guardava só no printer-settings.json (uma vez, se o banco não tiver nenhuma)
    match rt.block_on(barerp_db::printer::import_electron_file(&db, &data_dir.join("printer-settings.json"))) {
        Ok(true) => tracing::info!("impressoras trazidas do printer-settings.json do app antigo"),
        Ok(false) => {}
        Err(e) => tracing::warn!("printer-settings.json do app antigo não importado: {e}"),
    }

    // Inicia o servidor HTTP + Socket.IO para os APKs dos celulares na porta 3001
    let server = barerp_server::Server::new(db.clone(), data_dir, barerp_server::DEFAULT_PORT);
    let server_handle = server.handle();
    rt.spawn(async move {
        if let Err(e) = server.serve().await {
            tracing::warn!("servidor HTTP/Socket.IO encerrou: {e}");
        }
    });

    let ui = AppWindow::new()?;
    let _refresh = controller::start(&ui, db, rt.handle().clone(), path.display().to_string(), Some(server_handle));
    ui.run()?;
    Ok(())
}
