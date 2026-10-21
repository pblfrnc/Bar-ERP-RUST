//! Modais de apoio: categoria, histórico de preços, relatório de estoque, consulta de NCM e folha de balanço.

use chrono::Local;
use slint::SharedString;

use barerp_core::money::brl;
use barerp_db::products::{self, PriceChange};
use barerp_db::{digital_menu, DbResult};
use barerp_report::stock_sheet::{self, SheetOrder, StockCategory, StockProduct, StockSheetOptions};
use barerp_server::Corpo;

use super::{close_modal, emit, notify, num_text, reload, store, with_state};
use crate::ctx::{open_external, AppCtx};
use crate::view::model;
use crate::{AppWindow, CdHistory, CdModal, CdReportCat};

#[derive(Default)]
pub(crate) struct ToolsState {
    /// Categoria em edição (`None` = nova)
    pub editing_category: Option<String>,
    /// A categoria foi aberta pelo "+ Nova" do formulário do produto
    pub category_from_form: bool,
    /// Categorias na ordem da lista da folha de balanço (índice 0 = todas)
    pub sheet_categories: Vec<String>,
}

// ---------- Categoria ----------

fn open_category(ui: &AppWindow, id: Option<String>) {
    let st = store(ui);
    let (cat, suggested) = with_state(|s| {
        s.tools.editing_category = id.clone();
        let cat = id.as_ref().and_then(|id| s.categories.iter().find(|c| &c.id == id).cloned());
        (cat, products::suggest_code_start(&s.categories))
    });
    match cat {
        Some(c) => {
            st.set_c_editing(true);
            st.set_c_subtitle(format!(r#"Alterando dados de "{}""#, c.name).into());
            st.set_c_name(c.name.clone().into());
            st.set_c_code_start(c.code_start_or_default().to_string().into());
        }
        None => {
            st.set_c_editing(false);
            st.set_c_subtitle("Organize seus produtos e configure a faixa de códigos".into());
            st.set_c_name("".into());
            st.set_c_code_start(suggested.to_string().into());
        }
    }
    st.set_c_renumber(false);
    st.set_modal_error("".into());
    st.set_modal(CdModal::Category);
}

fn save_category(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let name = st.get_c_name().trim().to_string();
    if name.is_empty() {
        return;
    }
    let code_start = st.get_c_code_start().trim().parse::<i64>().ok();
    let renumber = st.get_c_renumber();
    let (editing, from_form) = with_state(|s| (s.tools.editing_category.clone(), s.tools.category_from_form));
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(
        async move {
            match editing {
                Some(id) => {
                    let (msg, events) = products::update_category(&db, &id, &name, None, code_start, renumber).await?;
                    DbResult::Ok((id, msg, events))
                }
                None => {
                    let (id, events) = products::create_category(&db, &name, Some("UtensilsCrossed"), code_start).await?;
                    Ok((id, None, events))
                }
            }
        },
        move |ui, result| match result {
            Ok((id, msg, events)) => {
                emit(&next, &events);
                if let Some(m) = msg {
                    notify(&next, m, false);
                }
                close_modal(ui);
                with_state(|s| s.tools.category_from_form = false);
                // Categoria criada pelo formulário do produto: já fica escolhida nele
                if from_form {
                    let db = next.db.clone();
                    next.spawn(async move { products::categories(&db).await }, move |ui, result| {
                        if let Ok(cats) = result {
                            with_state(|s| s.categories = cats);
                            super::editor::refresh_categories(ui, Some(&id));
                        }
                    });
                } else {
                    reload(&next);
                }
            }
            Err(e) => store(ui).set_modal_error(e.to_string().into()),
        },
    );
}

// ---------- Histórico de preços ----------

fn history_view(h: &PriceChange) -> CdHistory {
    let dir = |d: f64| if d > 0.005 { 1 } else if d < -0.005 { -1 } else { 0 };
    let cost_diff = h.cost_diff.or(match (h.new_cost_price, h.old_cost_price) {
        (Some(n), Some(o)) => Some(n - o),
        _ => None,
    });
    let price_diff = h.new_price - h.old_price;
    let pct = if h.price_percent != 0.0 { format!(" ({}{}%)", if h.price_percent > 0.0 { "+" } else { "" }, num_text(h.price_percent, 2)) } else { String::new() };
    CdHistory {
        date: h.created_at.0.with_timezone(&Local).format("%d/%m/%Y, %H:%M:%S").to_string().into(),
        by: if h.changed_by.is_empty() { "Operador".into() } else { h.changed_by.clone().into() },
        xml: h.reason == "IMPORT_XML",
        old_price: brl(h.old_price).into(),
        new_price: brl(h.new_price).into(),
        price_diff: match dir(price_diff) {
            1 => format!("▲ +{}{pct}", brl(price_diff)),
            -1 => format!("▼ -{}{pct}", brl(price_diff.abs())),
            _ => "Sem alteração de venda".into(),
        }
        .into(),
        price_dir: dir(price_diff),
        old_cost: h.old_cost_price.map(brl).unwrap_or_else(|| "-".into()).into(),
        new_cost: h.new_cost_price.map(brl).unwrap_or_else(|| "-".into()).into(),
        cost_diff: match cost_diff.map(dir) {
            Some(1) => format!("▲ Custo subiu (+{})", brl(cost_diff.unwrap_or(0.0))),
            Some(-1) => format!("▼ Custo caiu (-{})", brl(cost_diff.unwrap_or(0.0).abs())),
            _ => "Sem alteração de custo".into(),
        }
        .into(),
        cost_dir: cost_diff.map(dir).unwrap_or(0),
        chave: h.nfe_chave.clone().unwrap_or_default().into(),
    }
}

fn open_history(ctx: &AppCtx, id: String) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let subtitle = with_state(|s| {
        s.products
            .iter()
            .find(|p| p.id == id)
            .map(|p| match p.code.as_deref().filter(|c| !c.is_empty()) {
                Some(c) => format!("{} (#{c})", p.name),
                None => p.name.clone(),
            })
            .unwrap_or_default()
    });
    st.set_h_subtitle(subtitle.into());
    st.set_h_rows(model(Vec::new()));
    st.set_h_loading(true);
    st.set_modal(CdModal::History);
    let db = ctx.db.clone();
    ctx.spawn(async move { products::price_history(&db, &id).await }, |ui, result| {
        let st = store(ui);
        st.set_h_loading(false);
        st.set_h_rows(model(result.unwrap_or_default().iter().map(history_view).collect()));
    });
}

// ---------- Relatório de estoque ----------

fn open_report(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    st.set_r_loading(true);
    st.set_r_failed(false);
    st.set_modal(CdModal::Report);
    let db = ctx.db.clone();
    ctx.spawn(async move { products::stock_report(&db).await }, |ui, result| match result {
        Ok(r) => render_report(ui, &r),
        Err(_) => {
            store(ui).set_r_loading(false);
            store(ui).set_r_failed(true);
        }
    });
}

pub(crate) fn render_report(ui: &AppWindow, r: &products::StockReport) {
    {
        let st = store(ui);
        st.set_r_loading(false);
        st.set_r_failed(false);
        st.set_r_cost(brl(r.total_cost_value).into());
        st.set_r_cost_sub(format!("{} produtos cadastrados", r.total_products).into());
        st.set_r_sale(brl(r.total_sale_value).into());
        st.set_r_sale_sub(format!("{} unidades em estoque", r.total_stock_units).into());
        st.set_r_profit(brl(r.total_expected_profit).into());
        st.set_r_margin(format!("{}%", num_text(r.average_margin, 1)).into());
        st.set_r_count(format!("{} categorias", r.categories.len()).into());
        let rows: Vec<CdReportCat> = r
            .categories
            .iter()
            .map(|c| CdReportCat {
                name: c.category.clone().into(),
                items: c.items_count.to_string().into(),
                units: c.units_count.to_string().into(),
                cost: brl(c.total_cost).into(),
                sale: brl(c.total_sale).into(),
                profit: brl(c.expected_profit).into(),
                margin: format!("{}%", num_text(c.margin_percent, 1)).into(),
                tone: if c.margin_percent >= 50.0 { 0 } else if c.margin_percent >= 30.0 { 1 } else { 2 },
            })
            .collect();
        st.set_r_rows(model(rows));
    }
}

// ---------- Consulta de NCM ----------

fn search_ncm(ctx: &AppCtx, code: String) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let clean: String = code.chars().filter(|c| c.is_ascii_digit()).take(8).collect();
    st.set_n_code(clean.clone().into());
    if clean.len() < 4 {
        st.set_n_error("Digite ao menos 4 dígitos do NCM (ex: 22030000 para Cerveja).".into());
        return;
    }
    let Some(server) = ctx.server.clone() else {
        st.set_n_error("Consulta de NCM indisponível: o servidor interno não está rodando.".into());
        return;
    };
    st.set_n_loading(true);
    st.set_n_error("".into());
    st.set_n_has_result(false);
    ctx.spawn(async move { server.fiscal("GET", &format!("/ncm/{clean}"), Corpo::Nada).await }, |ui, r| {
        let st = store(ui);
        st.set_n_loading(false);
        let j = r.json();
        if r.ok() && j.get("valido").and_then(|v| v.as_bool()) == Some(true) {
            let text = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let pct = |k: &str, fallback: &str| j.get(k).and_then(|v| v.as_f64()).map(|v| format!("{}%", num_text(v, 2))).unwrap_or_else(|| fallback.into());
            st.set_n_result_code(text("codigo").into());
            let desc = text("descricao");
            st.set_n_result_desc(if desc.is_empty() { "Sem descrição cadastrada".into() } else { desc }.into());
            st.set_n_fed(pct("aliquotaNacional", "Conforme regime").into());
            st.set_n_est(pct("aliquotaEstadual", "Conforme estado").into());
            st.set_n_has_result(true);
        } else {
            let e = r.erro();
            st.set_n_error(if e.trim().is_empty() { "Código NCM não encontrado ou inválido na Receita Federal.".into() } else { e }.into());
        }
    });
}

// ---------- Folha de balanço ----------

fn sheet_inputs() -> (Vec<StockProduct>, Vec<StockCategory>) {
    with_state(|s| {
        let products = s
            .products
            .iter()
            .map(|p| StockProduct {
                code: p.code.clone(),
                name: p.name.clone(),
                category_id: p.category_id.clone(),
                unit: p.unit.clone(),
                stock: p.stock,
                track_stock: p.track_stock,
                price: p.price,
                cost_price: p.cost_price,
                has_box_price: p.has_box_price,
                box_quantity: p.box_quantity,
                box_price: p.box_price,
            })
            .collect();
        let categories = s
            .categories
            .iter()
            .map(|c| StockCategory { id: c.id.clone(), name: c.name.clone(), sort_order: c.sort_order })
            .collect();
        (products, categories)
    })
}

fn sheet_options(ui: &AppWindow, store_name: String) -> StockSheetOptions {
    let st = store(ui);
    let idx = st.get_s_category_index().max(0) as usize;
    let category_id = if idx == 0 { None } else { with_state(|s| s.tools.sheet_categories.get(idx - 1).cloned()) };
    StockSheetOptions {
        store_name,
        category_id,
        order: if st.get_s_alpha() { SheetOrder::Alpha } else { SheetOrder::Category },
        show_cost: st.get_s_show_cost(),
    }
}

fn sheet_count(ui: &AppWindow) {
    let (products, categories) = sheet_inputs();
    let opts = sheet_options(ui, String::new());
    let n: usize = stock_sheet::stock_sheet_groups(&products, &categories, &opts).iter().map(|g| g.products.len()).sum();
    store(ui).set_s_count(n as i32);
}

fn open_sheet(ui: &AppWindow) {
    let st = store(ui);
    let labels: Vec<SharedString> = with_state(|s| {
        let mut cats = s.categories.clone();
        cats.sort_by_key(|c| c.sort_order);
        s.tools.sheet_categories = cats.iter().map(|c| c.id.clone()).collect();
        std::iter::once("Todas as categorias".into()).chain(cats.iter().map(|c| c.name.clone().into())).collect()
    });
    st.set_s_category_labels(model(labels));
    st.set_s_category_index(0);
    st.set_s_alpha(true);
    st.set_s_show_cost(true);
    st.set_s_feedback("".into());
    st.set_s_busy(false);
    sheet_count(ui);
    st.set_modal(CdModal::Sheet);
}

fn export_sheet(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let now = Local::now().naive_local();
    let idx = st.get_s_category_index().max(0) as usize;
    let category_name = if idx == 0 {
        None
    } else {
        with_state(|s| {
            let id = s.tools.sheet_categories.get(idx - 1)?;
            s.categories.iter().find(|c| &c.id == id).map(|c| c.name.clone())
        })
    };
    let default_name = stock_sheet::stock_sheet_file_name(category_name.as_deref(), now);
    let Some(path) = rfd::FileDialog::new()
        .set_title("Salvar folha de balanço")
        .set_file_name(&default_name)
        .add_filter("PDF", &["pdf"])
        .save_file()
    else {
        return;
    };
    st.set_s_busy(true);
    st.set_s_feedback("".into());
    let (products, categories) = sheet_inputs();
    let opts_ui = sheet_options(&ui, String::new());
    let count = st.get_s_count();
    let db = ctx.db.clone();
    ctx.spawn(
        async move {
            let (_, restaurant) = digital_menu::title_sources(&db).await.unwrap_or((None, None));
            let opts = StockSheetOptions { store_name: restaurant.filter(|r| !r.trim().is_empty()).unwrap_or_else(|| "BarERP".into()), ..opts_ui };
            let pdf = stock_sheet::stock_sheet_pdf(&products, &categories, &opts, now);
            std::fs::write(&path, pdf).map(|_| path).map_err(|e| e.to_string())
        },
        move |ui, result| {
            let st = store(ui);
            st.set_s_busy(false);
            match result {
                Ok(path) => {
                    st.set_s_error(false);
                    st.set_s_feedback(format!("PDF gerado com {count} produtos.").into());
                    open_external(&path.to_string_lossy());
                }
                Err(e) => {
                    st.set_s_error(true);
                    st.set_s_feedback(format!("Erro ao gerar o PDF: {e}").into());
                }
            }
        },
    );
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    st.on_new_category({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                with_state(|s| s.tools.category_from_form = false);
                open_category(&ui, None);
            }
        }
    });
    st.on_open_category_from_form({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                with_state(|s| s.tools.category_from_form = true);
                open_category(&ui, None);
            }
        }
    });
    st.on_category_edit({
        let ctx = ctx.clone();
        move |id| {
            if let Some(ui) = ctx.ui() {
                with_state(|s| s.tools.category_from_form = false);
                open_category(&ui, Some(id.to_string()));
            }
        }
    });
    st.on_c_save({
        let ctx = ctx.clone();
        move || save_category(&ctx)
    });
    st.on_history({
        let ctx = ctx.clone();
        move |id| open_history(&ctx, id.to_string())
    });
    st.on_open_report({
        let ctx = ctx.clone();
        move || open_report(&ctx)
    });
    st.on_open_ncm({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                let st = store(&ui);
                st.set_n_code("".into());
                st.set_n_error("".into());
                st.set_n_has_result(false);
                st.set_n_loading(false);
                st.set_modal(CdModal::Ncm);
            }
        }
    });
    st.on_n_search({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                let code = store(&ui).get_n_code().to_string();
                search_ncm(&ctx, code);
            }
        }
    });
    st.on_n_quick({
        let ctx = ctx.clone();
        move |code| search_ncm(&ctx, code.to_string())
    });
    st.on_n_use({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                let st = store(&ui);
                st.set_f_ncm(st.get_n_result_code());
                close_modal(&ui);
                st.invoke_ncm_edited();
            }
        }
    });
    st.on_open_sheet({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                open_sheet(&ui);
            }
        }
    });
    st.on_s_changed({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                sheet_count(&ui);
            }
        }
    });
    st.on_s_export({
        let ctx = ctx.clone();
        move || export_sheet(&ctx)
    });
}
