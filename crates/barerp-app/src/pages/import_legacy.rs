//! "Importar do Sistema Antigo" (ImportXmlModal.tsx): aberto pelos botões "Importar XML" de Produtos e
//! Fornecedores (área Cardápio). As regras ficam em `barerp_db::legado`, as mesmas da rota `/api/import-xml/tabelas`.

use std::cell::RefCell;
use std::path::PathBuf;

use slint::{ComponentHandle, SharedString};

use barerp_db::legado;

use crate::ctx::AppCtx;
use crate::view::model;
use crate::{CardapioStore, ImportLegacyStore};

thread_local! {
    static FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn reset(store: &ImportLegacyStore) {
    store.set_file_name(SharedString::new());
    store.set_file_size(SharedString::new());
    store.set_result(SharedString::new());
    store.set_error(SharedString::new());
    store.set_importing(false);
    FILE.with(|f| *f.borrow_mut() = None);
}

pub fn open(ctx: &AppCtx) {
    if let Some(ui) = ctx.ui() {
        let store = ui.global::<ImportLegacyStore>();
        reset(&store);
        store.set_visible(true);
    }
}

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<ImportLegacyStore>();

    let c = ctx.clone();
    store.on_choose_file(move || {
        let Some(ui) = c.ui() else { return };
        let Some(path) = rfd::FileDialog::new().set_title("XML do sistema antigo").add_filter("XML", &["xml"]).pick_file() else {
            return;
        };
        let store = ui.global::<ImportLegacyStore>();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        store.set_file_name(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into());
        store.set_file_size(format!("{:.1} KB", size as f64 / 1024.0).into());
        store.set_result(SharedString::new());
        FILE.with(|f| *f.borrow_mut() = Some(path));
    });

    let c = ctx.clone();
    store.on_import(move || {
        let Some(ui) = c.ui() else { return };
        let Some(path) = FILE.with(|f| f.borrow().clone()) else { return };
        let store = ui.global::<ImportLegacyStore>();
        store.set_importing(true);
        store.set_result(SharedString::new());
        let db = c.db.clone();
        let c2 = c.clone();
        c.spawn(
            async move {
                let bytes = tokio::fs::read(&path).await.map_err(|e| format!("Não foi possível ler o arquivo: {e}"))?;
                legado::import_tables(&db, &bytes).await.map_err(|e| format!("Erro ao processar XML: {e}"))
            },
            move |ui, result| {
                let store = ui.global::<ImportLegacyStore>();
                store.set_importing(false);
                match result {
                    Ok((r, events)) => {
                        c2.emit(&events);
                        store.set_categories(r.categorias.importadas as i32);
                        store.set_suppliers(r.fornecedores.importados as i32);
                        store.set_suppliers_existing(r.fornecedores.duplicados as i32);
                        store.set_products(r.produtos.importados as i32);
                        store.set_products_updated(r.produtos.atualizados as i32);
                        store.set_bar(r.produtos.bar as i32);
                        store.set_kitchen(r.produtos.cozinha as i32);
                        store.set_errors(model(r.erros.into_iter().map(SharedString::from).collect()));
                        store.set_result("ok".into());
                        // Recarrega a lista de produtos/fornecedores da área Cardápio
                        ui.global::<CardapioStore>().invoke_shown();
                    }
                    Err(e) => {
                        store.set_error(e.into());
                        store.set_result("error".into());
                    }
                }
            },
        );
    });

    let c = ctx.clone();
    store.on_close(move || {
        if let Some(ui) = c.ui() {
            let store = ui.global::<ImportLegacyStore>();
            reset(&store);
            store.set_visible(false);
        }
    });
}
