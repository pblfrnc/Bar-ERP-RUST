//! Página "Cardápio & Estoque": port de ProductsView.tsx, SuppliersView.tsx, CustomersView.tsx e dos modais da
//! área (adicionais, promoções, folha de balanço, itens do cardápio digital, NCM, histórico, relatório).
//! Ver `paridade/cardapio.md` para a lista de funções do app original e o estado do port.

use std::cell::RefCell;
use std::sync::OnceLock;

use slint::{ComponentHandle, SharedString};

use barerp_core::money::brl;
use barerp_db::customers::Customer;
use barerp_db::events::Event;
use barerp_db::modifiers::GroupRow;
use barerp_db::products::{self, CategoryRow, ProductRow};
use barerp_db::suppliers::{self, Supplier};
use barerp_db::DbResult;
use barerp_menu_publish::db_store::DbMenuStore;
use barerp_menu_publish::MenuPublisher;

use crate::ctx::AppCtx;
use crate::view::model;
use crate::{AppWindow, CardapioStore, CdCategory, CdModal, CdProduct, CdScreen};

mod editor;
mod extras;
mod menu_items;
pub(crate) mod people;
mod tools;

/// Ação esperando o "sim" da confirmação.
#[derive(Clone)]
pub(crate) enum Confirm {
    DeleteProduct(String),
    DeleteCategory(String),
    Backfill,
    FixDuplicates,
    DeleteSupplier(String),
    DeleteGroup(String),
    DeletePromotion(String),
    CloseMenuItems,
}

/// Estado da página (só a thread da interface mexe).
#[derive(Default)]
pub(crate) struct State {
    pub products: Vec<ProductRow>,
    pub categories: Vec<CategoryRow>,
    pub suppliers: Vec<Supplier>,
    pub customers: Vec<Customer>,
    pub groups: Vec<GroupRow>,
    /// `None` = todas
    pub category_filter: Option<String>,
    pub confirm: Option<Confirm>,
    pub form: editor::Form,
    pub tools: tools::ToolsState,
    pub extras: extras::ExtrasState,
    pub menu: menu_items::MenuState,
    pub people: people::PeopleState,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub(crate) fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

// ---------- Publicação do cardápio digital ----------

static PUBLISHER: OnceLock<MenuPublisher<DbMenuStore>> = OnceLock::new();

/// Publicador do cardápio digital do app (um só). A área de Retaguarda usa o mesmo para status e configurações.
pub fn menu_publisher() -> Option<MenuPublisher<DbMenuStore>> {
    PUBLISHER.get().cloned()
}

/// Avisa os celulares e, se o cadastro mudou, agenda a publicação do cardápio (como o `router.use` do Node).
pub(crate) fn emit(ctx: &AppCtx, events: &[Event]) {
    ctx.emit(events);
    if events.iter().any(|e| matches!(e, Event::CatalogUpdated { .. })) {
        if let Some(p) = PUBLISHER.get() {
            let _guard = ctx.rt.enter();
            p.schedule_publish();
        }
    }
}

// ---------- Utilidades ----------

/// Número digitado com vírgula ou ponto.
pub(crate) fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Só dígitos, vírgula e ponto (os campos de valor do original).
pub(crate) fn keep_number_chars(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect()
}

/// Número para o campo, sem zeros sobrando e com vírgula: 9.5 → "9,5", 10.0 → "10".
pub(crate) fn num_text(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    s.replace('.', ",")
}

pub(crate) fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// CNPJ formatado quando tem 14 dígitos.
pub(crate) fn format_document(doc: &str) -> String {
    let d = digits(doc);
    if d.len() == 14 {
        format!("{}.{}.{}/{}-{}", &d[0..2], &d[2..5], &d[5..8], &d[8..12], &d[12..14])
    } else {
        doc.to_string()
    }
}

pub(crate) fn store(ui: &AppWindow) -> CardapioStore<'_> {
    ui.global::<CardapioStore>()
}

/// Abre a confirmação (o `window.confirm` do original).
pub(crate) fn ask(ui: &AppWindow, title: &str, text: &str, ok: &str, danger: bool, action: Confirm) {
    with_state(|s| s.confirm = Some(action));
    let st = store(ui);
    st.set_confirm_title(title.into());
    st.set_confirm_text(text.into());
    st.set_confirm_ok(ok.into());
    st.set_confirm_danger(danger);
    st.set_confirm_open(true);
}

/// Mensagem no rodapé do app (o `alert` do original).
pub(crate) fn notify(ctx: &AppCtx, message: impl Into<SharedString>, error: bool) {
    ctx.status(message, error);
}

pub(crate) fn close_modal(ui: &AppWindow) {
    let st = store(ui);
    st.set_modal(CdModal::None);
    st.set_modal_error(SharedString::new());
}

// ---------- Lista de produtos ----------

fn product_view(p: &ProductRow) -> CdProduct {
    let margin = p.margin_percent();
    CdProduct {
        id: p.id.clone().into(),
        name: p.name.clone().into(),
        code: p.code.as_deref().filter(|c| !c.trim().is_empty()).map(|c| format!("#{c}")).unwrap_or_default().into(),
        brand: p.brand.clone().unwrap_or_default().into(),
        supplier: p.supplier_label().unwrap_or("").into(),
        ean: p.ean.clone().unwrap_or_default().into(),
        description: p.description.clone().unwrap_or_default().into(),
        category: p.category_name.clone().unwrap_or_else(|| "Geral".into()).into(),
        kds: p.kds_station.clone().into(),
        price: brl(p.price).into(),
        r#box: match (p.has_box_price, p.box_price) {
            (true, Some(bp)) if bp > 0.0 => format!("Cx {}x: {}", p.box_quantity.unwrap_or(24), brl(bp)),
            _ => String::new(),
        }
        .into(),
        cost: p.cost_price.filter(|c| *c > 0.0).map(brl).unwrap_or_else(|| "-".into()).into(),
        margin: margin.map(|m| format!("{m}% margem")).unwrap_or_default().into(),
        margin_tone: match margin {
            Some(m) if m >= 50 => 0,
            Some(m) if m >= 30 => 1,
            _ => 2,
        },
        stock: format!("{} {}", p.stock, p.unit).into(),
        low: p.is_low_stock(),
        composed: p.component_count > 0,
    }
}

fn is_barcode(s: &str) -> bool {
    let t = s.trim();
    (7..=14).contains(&t.len()) && t.chars().all(|c| c.is_ascii_digit())
}

/// Lista com os filtros da tela (categoria e busca).
pub(crate) fn render_list(ui: &AppWindow) {
    let st = store(ui);
    let query = st.get_search().trim().to_lowercase();
    with_state(|s| {
        let has = |v: &Option<String>| v.as_deref().is_some_and(|t| t.to_lowercase().contains(&query));
        let visible: Vec<CdProduct> = s
            .products
            .iter()
            .filter(|p| s.category_filter.as_ref().is_none_or(|c| &p.category_id == c))
            .filter(|p| {
                query.is_empty()
                    || p.name.to_lowercase().contains(&query)
                    || has(&p.code)
                    || has(&p.ean)
                    || has(&p.brand)
                    || has(&p.supplier)
                    || has(&p.supplier_name)
                    || has(&p.description)
            })
            .map(product_view)
            .collect();
        st.set_products(model(visible));
        st.set_total_count(s.products.len() as i32);
        st.set_duplicate_count(products::duplicate_code_count(&s.products) as i32);
        st.set_supplier_count(s.suppliers.len() as i32);
        st.set_all_categories(s.category_filter.is_none());
        let cats: Vec<CdCategory> = s
            .categories
            .iter()
            .map(|c| CdCategory {
                id: c.id.clone().into(),
                name: c.name.clone().into(),
                range: format!("#{}+", c.code_start_or_default()).into(),
                active: s.category_filter.as_deref() == Some(c.id.as_str()),
            })
            .collect();
        st.set_categories(model(cats));
    });
    st.set_search_is_barcode(is_barcode(&st.get_search()));
}

/// Recarrega produtos, categorias, fornecedores e grupos de adicionais.
pub(crate) fn reload(ctx: &AppCtx) {
    let db = ctx.db.clone();
    if let Some(ui) = ctx.ui() {
        store(&ui).set_loading(true);
    }
    ctx.spawn(
        async move {
            let products = products::list(&db).await?;
            let categories = products::categories(&db).await?;
            let suppliers = suppliers::list(&db, None).await?;
            let groups = barerp_db::modifiers::list(&db).await?;
            DbResult::Ok((products, categories, suppliers, groups))
        },
        |ui, result| {
            store(ui).set_loading(false);
            match result {
                Ok((products, categories, suppliers, groups)) => {
                    with_state(|s| {
                        if s.category_filter.as_ref().is_some_and(|c| !categories.iter().any(|x| &x.id == c)) {
                            s.category_filter = None;
                        }
                        s.products = products;
                        s.categories = categories;
                        s.suppliers = suppliers;
                        s.groups = groups;
                    });
                    render_list(ui);
                    people::render_suppliers(ui);
                }
                Err(e) => tracing::warn!("erro ao carregar o cardápio: {e}"),
            }
        },
    );
}

/// Ação da lista com mensagem de resultado (organizar, gerar códigos, corrigir duplicados...).
fn run_with_message<F>(ctx: &AppCtx, busy: fn(&AppWindow, bool), fut: F)
where
    F: std::future::Future<Output = DbResult<(String, Vec<Event>)>> + Send + 'static,
{
    if let Some(ui) = ctx.ui() {
        busy(&ui, true);
    }
    let next = ctx.clone();
    ctx.spawn(fut, move |ui, result| {
        busy(ui, false);
        match result {
            Ok((message, events)) => {
                emit(&next, &events);
                notify(&next, message, false);
                reload(&next);
            }
            Err(e) => notify(&next, format!("Erro: {e}"), true),
        }
    });
}

fn confirm_yes(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    store(&ui).set_confirm_open(false);
    let Some(action) = with_state(|s| s.confirm.take()) else { return };
    let db = ctx.db.clone();
    match action {
        Confirm::DeleteProduct(id) => {
            let next = ctx.clone();
            ctx.spawn(async move { products::delete(&db, &id).await }, move |ui, result| match result {
                Ok((message, events)) => {
                    emit(&next, &events);
                    notify(&next, message, false);
                    if store(ui).get_screen() == CdScreen::Editor {
                        store(ui).set_screen(CdScreen::Products);
                    }
                    reload(&next);
                }
                Err(e) => notify(&next, e.to_string(), true),
            });
        }
        Confirm::DeleteCategory(id) => {
            let next = ctx.clone();
            ctx.spawn(async move { products::delete_category(&db, &id).await }, move |_, result| match result {
                Ok((message, events)) => {
                    emit(&next, &events);
                    notify(&next, message, false);
                    reload(&next);
                }
                Err(e) => notify(&next, e.to_string(), true),
            });
        }
        Confirm::Backfill => run_with_message(ctx, |ui, b| store(ui).set_backfilling(b), async move { products::backfill_codes(&db).await }),
        Confirm::FixDuplicates => {
            run_with_message(ctx, |ui, b| store(ui).set_fixing(b), async move { products::fix_duplicate_codes(&db, false).await })
        }
        Confirm::DeleteSupplier(id) => people::delete_supplier(ctx, id),
        Confirm::DeleteGroup(id) => extras::delete_group(ctx, id),
        Confirm::DeletePromotion(id) => extras::delete_promotion(ctx, id),
        Confirm::CloseMenuItems => menu_items::close(ctx),
    }
}

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };

    // Publicador do cardápio digital: confere 20s depois de abrir e a cada 5 minutos
    let publisher = PUBLISHER.get_or_init(|| MenuPublisher::new(DbMenuStore::new(ctx.db.clone())));
    {
        let _guard = ctx.rt.enter();
        publisher.spawn_scheduler();
    }

    let st = store(&ui);
    st.on_shown({
        let ctx = ctx.clone();
        move || reload(&ctx)
    });
    st.on_show_screen({
        let ctx = ctx.clone();
        move |screen| {
            let Some(ui) = ctx.ui() else { return };
            store(&ui).set_screen(screen);
            match screen {
                CdScreen::Suppliers => people::open_suppliers(&ctx),
                CdScreen::Customers => people::open_customers(&ctx),
                _ => reload(&ctx),
            }
        }
    });
    st.on_search_changed({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render_list(&ui);
            }
        }
    });
    st.on_category_picked({
        let ctx = ctx.clone();
        move |id| {
            with_state(|s| s.category_filter = if id == "ALL" { None } else { Some(id.to_string()) });
            if let Some(ui) = ctx.ui() {
                render_list(&ui);
            }
        }
    });
    st.on_category_delete({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let name = with_state(|s| s.categories.iter().find(|c| c.id == id.as_str()).map(|c| c.name.clone())).unwrap_or_default();
            ask(
                &ui,
                "Excluir categoria",
                &format!(
                    r#"Deseja realmente EXCLUIR a categoria "{name}"? Os produtos dela que nunca foram vendidos também serão apagados."#
                ),
                "Excluir",
                true,
                Confirm::DeleteCategory(id.to_string()),
            );
        }
    });
    st.on_delete_product({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let name = with_state(|s| s.products.iter().find(|p| p.id == id.as_str()).map(|p| p.name.clone())).unwrap_or_default();
            ask(
                &ui,
                "Excluir produto",
                &format!(r#"Deseja realmente EXCLUIR o produto "{name}" do cardápio? Esta ação não pode ser desfeita."#),
                "Excluir",
                true,
                Confirm::DeleteProduct(id.to_string()),
            );
        }
    });
    st.on_quick_stock({
        let ctx = ctx.clone();
        move |id, delta| {
            let db = ctx.db.clone();
            let next = ctx.clone();
            let id = id.to_string();
            ctx.spawn(async move { products::adjust_stock(&db, &id, delta as i64).await }, move |_, result| match result {
                Ok((_, events)) => {
                    emit(&next, &events);
                    reload(&next);
                }
                Err(e) => notify(&next, e.to_string(), true),
            });
        }
    });
    st.on_organize({
        let ctx = ctx.clone();
        move || {
            let db = ctx.db.clone();
            run_with_message(&ctx, |ui, b| store(ui).set_organizing(b), async move { products::organize_bar_kitchen(&db).await });
        }
    });
    st.on_backfill({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                ask(
                    &ui,
                    "Gerar códigos internos",
                    "Atribuir códigos internos sequenciais a todos os produtos que não têm código?\n\nEsta operação não altera produtos que já têm código.",
                    "Gerar Códigos",
                    false,
                    Confirm::Backfill,
                );
            }
        }
    });
    st.on_fix_duplicates({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                ask(
                    &ui,
                    "Corrigir numeração",
                    "Deseja corrigir todos os produtos com numeração duplicada ou sem código, gerando novos números sequenciais exclusivos?",
                    "Corrigir",
                    false,
                    Confirm::FixDuplicates,
                );
            }
        }
    });
    st.on_import_xml({
        let ctx = ctx.clone();
        move || crate::pages::import_legacy::open(&ctx)
    });
    st.on_open_suppliers({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                store(&ui).set_screen(CdScreen::Suppliers);
                people::open_suppliers(&ctx);
            }
        }
    });
    st.on_open_customers({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                store(&ui).set_screen(CdScreen::Customers);
                people::open_customers(&ctx);
            }
        }
    });
    st.on_confirm_yes({
        let ctx = ctx.clone();
        move || confirm_yes(&ctx)
    });
    st.on_confirm_no({
        let ctx = ctx.clone();
        move || {
            with_state(|s| s.confirm = None);
            if let Some(ui) = ctx.ui() {
                store(&ui).set_confirm_open(false);
            }
        }
    });
    st.on_close_modal({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                close_modal(&ui);
            }
        }
    });

    editor::bind(ctx);
    tools::bind(ctx);
    extras::bind(ctx);
    menu_items::bind(ctx);
    people::bind(ctx);
}

/// Ids e textos dos fornecedores para a escolha do produto ("Nenhum fornecedor vinculado" primeiro).
pub(crate) fn supplier_choices(suppliers: &[Supplier]) -> (Vec<String>, Vec<SharedString>) {
    let mut ids = Vec::new();
    let mut labels: Vec<SharedString> = vec!["Nenhum fornecedor vinculado".into()];
    for s in suppliers {
        ids.push(s.id.clone());
        let name = match s.trade_name.as_deref().filter(|t| !t.is_empty()) {
            Some(t) => format!("{t} ({})", s.name),
            None => s.name.clone(),
        };
        let doc = s.document.as_deref().map(|d| format!(" • {}", format_document(d))).unwrap_or_default();
        labels.push(format!("{name}{doc}").into());
    }
    (ids, labels)
}

/// Prints da área sem banco assíncrono (exemplo `cardapio_shots`): preenche as telas com dados prontos.
pub mod preview {
    use barerp_db::modifiers::GroupRow;
    use barerp_db::products::{CategoryRow, ProductDetail, ProductRow, StockReport};
    use barerp_db::suppliers::Supplier;

    use super::{editor, extras, menu_items, people, render_list, store, tools, with_state};
    use crate::{AppWindow, CdModal, CdScreen};

    pub fn list(ui: &AppWindow, products: Vec<ProductRow>, categories: Vec<CategoryRow>, suppliers: Vec<Supplier>, groups: Vec<GroupRow>) {
        with_state(|s| {
            s.products = products;
            s.categories = categories;
            s.suppliers = suppliers;
            s.groups = groups;
        });
        close(ui);
        store(ui).set_screen(CdScreen::Products);
        render_list(ui);
    }

    pub fn editor(ui: &AppWindow, detail: &ProductDetail) {
        close(ui);
        editor::fill_from(ui, detail, false);
        editor::show_editor(ui);
    }

    pub fn report(ui: &AppWindow, report: &StockReport) {
        store(ui).set_modal(CdModal::Report);
        tools::render_report(ui, report);
    }

    pub fn modifiers(ui: &AppWindow) {
        extras::render_groups(ui);
        store(ui).set_m_draft(false);
        store(ui).set_modal(CdModal::Modifiers);
    }

    pub fn menu_items(ui: &AppWindow) {
        menu_items::start(ui);
    }

    pub fn suppliers(ui: &AppWindow) {
        close(ui);
        store(ui).set_screen(CdScreen::Suppliers);
        people::render_suppliers(ui);
    }

    pub fn close(ui: &AppWindow) {
        store(ui).set_modal(CdModal::None);
        store(ui).set_confirm_open(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeros_dos_campos() {
        assert_eq!(parse_num("4,50"), Some(4.5));
        assert_eq!(parse_num(" 10 "), Some(10.0));
        assert_eq!(parse_num(""), None);
        assert_eq!(num_text(9.5, 2), "9,5");
        assert_eq!(num_text(10.0, 2), "10");
        assert_eq!(num_text(0.125, 4), "0,125");
        assert_eq!(keep_number_chars("R$ 1.234,5a"), "1.234,5");
        assert_eq!(format_document("07526557000100"), "07.526.557/0001-00");
        assert!(is_barcode("7896045506040") && !is_barcode("123") && !is_barcode("78960a5506040"));
    }
}
