// Sem janela de console no Windows (o app é aberto pelo atalho, como qualquer programa)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! BarERP Pro nativo: telas em Slint, banco SQLite local (o mesmo `bar.db` da versão Electron).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::runtime::Runtime;

use barerp_app::{controller, AppWindow};
use barerp_db::Db;

/// Pastas do `userData` da versão Electron em `%APPDATA%`, na ordem de preferência.
const DB_FOLDERS: [&str; 3] = ["BarERP Pro", "bar-erp", "BarERP"];

/// Onde fica o banco. A versão Electron grava no `userData` do app em `%APPDATA%`; procuramos os nomes
/// possíveis da pasta para abrir o banco do cliente. `BARERP_DB` força outro caminho (testes).
/// Com mais de um `bar.db` (por exemplo, um vazio criado quando a pasta certa ficou sem banco por um momento),
/// pergunta qual abrir em vez de abrir o primeiro em silêncio. `None`: a janela foi fechada sem escolher.
fn db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("BARERP_DB") {
        return Some(PathBuf::from(path));
    }
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    let found: Vec<PathBuf> =
        DB_FOLDERS.iter().map(|folder| base.join(folder).join("bar.db")).filter(|p| p.exists()).collect();
    match found.len() {
        0 => Some(base.join(DB_FOLDERS[0]).join("bar.db")),
        1 => found.into_iter().next(),
        _ => choose_db(&found),
    }
}

slint::slint! {
    import { Button, VerticalBox } from "std-widgets.slint";

    export struct DbOption { path: string, info: string, recommended: bool }

    export component DbPicker inherits Window {
        title: "BarERP Pro";
        preferred-width: 680px;
        preferred-height: 420px;
        in property <[DbOption]> options;
        callback pick(int);
        VerticalBox {
            Text {
                text: "Este computador tem mais de um banco de dados do BarERP.";
                font-size: 16px;
                font-weight: 700;
                wrap: word-wrap;
            }
            Text {
                text: "Escolha qual abrir. O maior costuma ser o que tem as mesas, os produtos e as vendas do bar. "
                    + "Para não ver mais esta pergunta, feche o BarERP e renomeie a pasta do banco que não é usado.";
                wrap: word-wrap;
            }
            for option[i] in root.options: HorizontalLayout {
                spacing: 12px;
                VerticalLayout {
                    horizontal-stretch: 1;
                    alignment: center;
                    Text { text: option.path; wrap: word-wrap; font-weight: option.recommended ? 700 : 400; }
                    Text { text: option.info; color: #64748b; }
                }
                Button {
                    text: option.recommended ? "Abrir (recomendado)" : "Abrir";
                    primary: option.recommended;
                    clicked => { root.pick(i); }
                }
            }
        }
    }
}

/// Tamanho do banco somado ao do `-wal` (as alterações recentes podem estar só nele) e a data da última alteração.
fn db_stats(path: &Path) -> (u64, Option<std::time::SystemTime>) {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let (mut size, mut modified) = (0, None);
    for file in [path, Path::new(&wal)] {
        if let Ok(meta) = std::fs::metadata(file) {
            size += meta.len();
            modified = modified.max(meta.modified().ok());
        }
    }
    (size, modified)
}

fn choose_db(found: &[PathBuf]) -> Option<PathBuf> {
    let stats: Vec<_> = found.iter().map(|p| db_stats(p)).collect();
    let biggest = (0..found.len()).max_by_key(|&i| stats[i].0).unwrap_or(0);
    tracing::warn!("mais de um bar.db encontrado: {found:?}");
    let options: Vec<DbOption> = found
        .iter()
        .zip(&stats)
        .enumerate()
        .map(|(i, (path, (size, modified)))| {
            let size = if *size >= 1_048_576 {
                format!("{:.1} MB", *size as f64 / 1_048_576.0)
            } else {
                format!("{} KB", size.div_ceil(1024))
            };
            let when = modified
                .map(|t| chrono::DateTime::<chrono::Local>::from(t).format(" · alterado em %d/%m/%Y %H:%M").to_string())
                .unwrap_or_default();
            DbOption { path: path.display().to_string().into(), info: format!("{size}{when}").into(), recommended: i == biggest }
        })
        .collect();
    // Sem janela (erro do sistema gráfico), o app principal também não abriria: fica com o maior
    let Ok(picker) = DbPicker::new() else { return Some(found[biggest].clone()) };
    picker.set_options(ModelRc::new(VecModel::from(options)));
    let chosen = Rc::new(Cell::new(None));
    picker.on_pick({
        let chosen = chosen.clone();
        move |i| {
            chosen.set(Some(i as usize));
            let _ = slint::quit_event_loop();
        }
    });
    if picker.run().is_err() {
        return Some(found[biggest].clone());
    }
    let path = found[chosen.get()?].clone();
    tracing::info!("banco escolhido: {}", path.display());
    Some(path)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let Some(path) = db_path() else { return };
    // Sem console no Windows: um erro ao abrir (banco travado, pasta sem permissão) precisa aparecer numa janela,
    // senão o app simplesmente não abre
    if let Err(e) = run(&path) {
        tracing::error!("o BarERP não conseguiu abrir: {e}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("BarERP Pro")
            .set_description(format!("O BarERP não conseguiu abrir.\n\n{e}\n\nBanco de dados: {}", path.display()))
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        std::process::exit(1);
    }
}

fn run(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let rt = Runtime::new()?;
    let data_dir = path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir)?;
    }
    tracing::info!("banco de dados: {}", path.display());
    let db = rt.block_on(Db::open(path)).map_err(|e| format!("Erro ao abrir o banco de dados: {e}"))?;

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
