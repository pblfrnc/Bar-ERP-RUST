//! Cadastro / edição do produto em tela inteira (a parte `showProductModal` do ProductsView.tsx).

use std::sync::OnceLock;

use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString};

use barerp_catalog::{conversion, photo, Catalog, LookupMode};
use barerp_core::money::brl;
use barerp_core::pricing::round2;
use barerp_db::digital_menu;
use barerp_db::modifiers::{self, describe_rule};
use barerp_db::products::{self, ProductDetail, ProductInput};
use barerp_db::{suppliers, DbResult};
use barerp_menu_publish::db_store::save_product_image;
use barerp_server::Corpo;

use super::{
    digits, emit, format_document, keep_number_chars, notify, num_text, parse_num, reload, store, supplier_choices, with_state,
    State,
};
use crate::ctx::{open_external, AppCtx};
use crate::view::model;
use crate::{AppWindow, CdComponent, CdGroupPick, CdModal, CdScreen};

/// Valores antes da conversão (para "Desfazer").
#[derive(Clone, Default)]
pub(crate) struct ConvBackup {
    stock: String,
    cost: String,
    price: String,
    factor: u32,
    has_box: bool,
    box_qty: String,
    box_price: String,
}

#[derive(Default)]
pub(crate) struct Form {
    /// Produto em edição (`None` = novo)
    pub editing: Option<String>,
    /// Ficha técnica: (insumo, quantidade digitada)
    pub components: Vec<(Option<String>, String)>,
    pub group_ids: Vec<String>,
    pub initial_group_ids: Vec<String>,
    /// Ids na ordem das listas da tela
    pub category_ids: Vec<String>,
    pub supplier_ids: Vec<String>,
    pub component_ids: Vec<String>,
    /// Foto nova (já reduzida) a gravar depois de salvar
    pub photo_new: Option<Vec<u8>>,
    pub photo_removed: bool,
    pub had_photo: bool,
    pub conv_backup: Option<ConvBackup>,
    pub ncm_seq: u64,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

fn catalog(ctx: &AppCtx) -> &'static Catalog {
    CATALOG.get_or_init(|| Catalog::new(ctx.db.clone()))
}

pub(crate) fn image_from(data: &[u8], side: u32) -> Option<Image> {
    let (w, h, px) = photo::thumbnail_rgba(data, side)?;
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, w, h);
    Some(Image::from_rgba8(buffer))
}

// ---------- Cálculos ao vivo ----------

/// Atualiza simulador de margem, caixa, custo da receita, rodapé e cartão do fornecedor.
pub(crate) fn refresh_derived(ui: &AppWindow) {
    let st = store(ui);
    let price = parse_num(&st.get_f_price()).unwrap_or(0.0);
    let cost = parse_num(&st.get_f_cost()).unwrap_or(0.0);
    let profit = price - cost;
    let margin = if price > 0.0 { (price - cost) / price * 100.0 } else { 0.0 };
    st.set_profit(brl(profit).into());
    st.set_profit_negative(profit < 0.0);
    st.set_margin_label(if price > 0.0 { format!("{}%", num_fixed(margin, 1)) } else { "0,0%".into() }.into());
    st.set_margin_tone(if margin >= 50.0 { 0 } else if margin >= 25.0 { 1 } else if margin > 0.0 { 2 } else { 3 });
    st.set_markup(if cost > 0.0 { format!("+{}%", num_fixed((price - cost) / cost * 100.0, 1)) } else { "—".into() }.into());
    st.set_footer_sale(brl(price).into());
    st.set_footer_cost(if cost > 0.0 { brl(cost) } else { String::new() }.into());
    st.set_footer_margin(if price > 0.0 && cost > 0.0 { format!("{}%", num_fixed(margin, 1)) } else { String::new() }.into());
    st.set_footer_margin_tone(if margin >= 50.0 { 0 } else if margin >= 20.0 { 1 } else { 2 });

    let qty = st.get_f_box_qty().trim().parse::<i64>().unwrap_or(0);
    let box_price = parse_num(&st.get_f_box_price()).unwrap_or(0.0);
    st.set_box_unit_label(
        if qty > 0 && box_price > 0.0 {
            let unit = box_price / qty as f64;
            let diff = price - unit;
            let saving = if diff > 0.0 { format!(" (Economia de {}/un)", brl(diff)) } else { String::new() };
            format!("Sai a {} por unidade{saving}", brl(unit))
        } else {
            String::new()
        }
        .into(),
    );

    let (recipe, rows) = with_state(|s| {
        let mut total = 0.0;
        let rows: Vec<CdComponent> = s
            .form
            .components
            .iter()
            .map(|(id, qty_text)| {
                let product = id.as_ref().and_then(|id| s.products.iter().find(|p| &p.id == id));
                let qty = parse_num(qty_text).unwrap_or(0.0);
                let unit_cost = product.map(|p| p.cost_price.filter(|c| *c > 0.0).unwrap_or(p.price)).unwrap_or(0.0);
                let subtotal = unit_cost * qty;
                total += subtotal;
                CdComponent {
                    index: id
                        .as_ref()
                        .and_then(|id| s.form.component_ids.iter().position(|x| x == id))
                        .map(|i| i as i32 + 1)
                        .unwrap_or(0),
                    quantity: qty_text.clone().into(),
                    unit: product.map(|p| p.unit.clone()).unwrap_or_else(|| "un".into()).into(),
                    subtotal: if subtotal > 0.0 { brl(subtotal) } else { String::new() }.into(),
                }
            })
            .collect();
        (total, rows)
    });
    st.set_recipe_cost(if recipe > 0.0 { brl(recipe) } else { String::new() }.into());
    let current = st.get_f_components();
    if current.row_count() == rows.len() {
        for (i, r) in rows.into_iter().enumerate() {
            if current.row_data(i).as_ref() != Some(&r) {
                current.set_row_data(i, r);
            }
        }
    } else {
        st.set_f_components(model(rows));
    }

    // Cartão do fornecedor escolhido
    let idx = st.get_f_supplier_index();
    let sup = with_state(|s| {
        if idx <= 0 {
            return None;
        }
        let id = s.form.supplier_ids.get(idx as usize - 1)?;
        s.suppliers.iter().find(|x| &x.id == id).cloned()
    });
    let sup = sup.unwrap_or_else(|| suppliers::Supplier {
        id: String::new(),
        name: String::new(),
        trade_name: None,
        document: None,
        ie: None,
        phone: None,
        email: None,
        city: None,
        state: None,
        address: None,
        contact_name: None,
        notes: None,
        created_at: barerp_db::DbTime::now(),
        product_count: 0,
    });
    st.set_sup_card_title(sup.trade_name.clone().filter(|t| !t.is_empty()).unwrap_or(sup.name.clone()).into());
    st.set_sup_card_doc(sup.document.as_deref().map(format_document).unwrap_or_default().into());
    st.set_sup_card_contact(sup.contact_name.clone().unwrap_or_default().into());
    st.set_sup_card_phone(sup.phone.clone().unwrap_or_default().into());
}

/// `x.toFixed(n)` com vírgula.
fn num_fixed(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}").replace('.', ",")
}

fn render_groups(ui: &AppWindow) {
    let picks: Vec<CdGroupPick> = with_state(|s| {
        s.groups
            .iter()
            .map(|g| {
                let active_opts = g.options.iter().filter(|o| o.is_active).count();
                let rule = if g.is_active { describe_rule(g.min_select, g.max_select) } else { "Grupo inativo".into() };
                CdGroupPick {
                    id: g.id.clone().into(),
                    name: g.name.clone().into(),
                    detail: format!("{rule} · {active_opts} opções").into(),
                    position: s.form.group_ids.iter().position(|x| x == &g.id).map(|p| p as i32 + 1).unwrap_or(0),
                    active: g.is_active,
                }
            })
            .collect()
    });
    store(ui).set_f_groups(model(picks));
}

/// Listas de escolha do formulário (categorias, fornecedores, insumos).
fn fill_choices(ui: &AppWindow) {
    let st = store(ui);
    let (cat_labels, sup_labels, comp_labels) = with_state(|s| {
        s.form.category_ids = s.categories.iter().map(|c| c.id.clone()).collect();
        let cats: Vec<SharedString> = s
            .categories
            .iter()
            .map(|c| format!("{} (Faixa #{})", c.name, c.code_start_or_default()).into())
            .collect();
        let (sup_ids, sups) = supplier_choices(&s.suppliers);
        s.form.supplier_ids = sup_ids;
        let editing = s.form.editing.clone();
        let insumos: Vec<&products::ProductRow> =
            s.products.iter().filter(|p| Some(&p.id) != editing.as_ref() && p.component_count == 0).collect();
        s.form.component_ids = insumos.iter().map(|p| p.id.clone()).collect();
        let mut comps: Vec<SharedString> = vec!["Selecione um insumo do estoque...".into()];
        comps.extend(insumos.iter().map(|p| {
            let cost = p.cost_price.filter(|c| *c > 0.0).map(|c| format!(" • Custo {}", brl(c))).unwrap_or_default();
            SharedString::from(format!("{} (Estoque: {} {}){cost}", p.name, p.stock, p.unit))
        }));
        (cats, sups, comps)
    });
    st.set_f_category_labels(model(cat_labels));
    st.set_f_supplier_labels(model(sup_labels));
    st.set_f_component_labels(model(comp_labels));
}

/// Recarrega a lista de categorias do formulário e escolhe `select` (categoria criada pelo "+ Nova").
pub(crate) fn refresh_categories(ui: &AppWindow, select: Option<&str>) {
    let current = with_state(|s| s.form.category_ids.get(store(ui).get_f_category_index().max(0) as usize).cloned());
    fill_choices(ui);
    if let Some(target) = select.map(str::to_string).or(current) {
        store(ui).set_f_category_index(category_index(&target));
    }
}

fn kds_index(station: &str) -> i32 {
    match station {
        "KITCHEN" => 1,
        "NONE" => 2,
        _ => 0,
    }
}

fn kds_of(index: i32) -> &'static str {
    match index {
        1 => "KITCHEN",
        2 => "NONE",
        _ => "BAR",
    }
}

fn category_index(id: &str) -> i32 {
    with_state(|s| s.form.category_ids.iter().position(|c| c == id).map(|i| i as i32).unwrap_or(0))
}

fn supplier_index(id: Option<&str>) -> i32 {
    let Some(id) = id else { return 0 };
    with_state(|s| s.form.supplier_ids.iter().position(|c| c == id).map(|i| i as i32 + 1).unwrap_or(0))
}

/// Limpa o formulário (novo produto).
fn reset(ui: &AppWindow) {
    with_state(|s| {
        s.form = Form::default();
    });
    fill_choices(ui);
    let st = store(ui);
    st.set_f_ean("".into());
    st.set_f_name("".into());
    st.set_f_code("".into());
    st.set_f_brand("".into());
    st.set_f_unit("un".into());
    st.set_f_cost("".into());
    st.set_f_margin("50".into());
    st.set_f_price("".into());
    st.set_f_has_box(false);
    st.set_f_box_qty("24".into());
    st.set_f_box_price("".into());
    st.set_f_box_ean("".into());
    st.set_f_conv_factor("12".into());
    st.set_f_conv_portion(false);
    st.set_f_composed(false);
    st.set_f_show_in_menu(true);
    st.set_f_description("".into());
    st.set_f_supplier_name("".into());
    st.set_f_supplier_index(0);
    st.set_f_kds_index(0);
    st.set_f_stock("100".into());
    st.set_f_min_stock("10".into());
    st.set_f_ncm("".into());
    st.set_f_cest("".into());
    st.set_f_cfop("5102".into());
    st.set_f_has_photo(false);
    st.set_f_photo(Image::default());
    st.set_lookup_message("".into());
    st.set_lookup_tone(0);
    st.set_lookup_busy("".into());
    st.set_conv_message("".into());
    st.set_ncm_state(0);
    st.set_ncm_detail("".into());
}

pub(crate) fn show_editor(ui: &AppWindow) {
    render_groups(ui);
    refresh_derived(ui);
    store(ui).set_screen(CdScreen::Editor);
}

/// Novo produto (categoria do filtro ou a primeira, com o próximo código da faixa).
pub(crate) fn open_new(ctx: &AppCtx, ean: Option<String>) {
    let Some(ui) = ctx.ui() else { return };
    reset(&ui);
    let st = store(&ui);
    st.set_f_editing(false);
    st.set_f_title("Cadastrar Novo Produto".into());
    st.set_f_badge("NOVO ITEM".into());
    let target = with_state(|s| s.category_filter.clone().or_else(|| s.categories.first().map(|c| c.id.clone())));
    if let Some(cat) = &target {
        st.set_f_category_index(category_index(cat));
        load_next_code(ctx, cat.clone());
    }
    show_editor(&ui);
    if let Some(ean) = ean {
        st.set_f_ean(ean.into());
        lookup(ctx, LookupMode::All);
    }
}

fn load_next_code(ctx: &AppCtx, category_id: String) {
    let db = ctx.db.clone();
    ctx.spawn(async move { products::next_code(&db, &category_id).await }, |ui, result| {
        if let Ok(code) = result {
            store(ui).set_f_code(code.into());
        }
    });
}

/// Preenche o formulário com um produto (editar ou duplicar).
pub(crate) fn fill_from(ui: &AppWindow, d: &ProductDetail, duplicate: bool) {
    reset(ui);
    let p = &d.product;
    with_state(|s| {
        s.form.editing = if duplicate { None } else { Some(p.id.clone()) };
        s.form.components = d.components.iter().map(|c| (Some(c.component_id.clone()), num_text(c.quantity, 4))).collect();
        s.form.group_ids = d.modifier_group_ids.clone();
        s.form.initial_group_ids = if duplicate { Vec::new() } else { d.modifier_group_ids.clone() };
        s.form.had_photo = !duplicate && p.image_hash.is_some();
    });
    fill_choices(ui);
    let st = store(ui);
    st.set_f_editing(!duplicate);
    if duplicate {
        st.set_f_title("Cadastrar Novo Produto".into());
        st.set_f_badge("NOVO ITEM".into());
        st.set_f_name(format!("{} (Cópia)", p.name).into());
        st.set_f_ean("".into());
        st.set_lookup_message(
            format!(
                r#"Item duplicado a partir de "{}". Novo código interno sequencial gerado! Ajuste o nome e bipe o novo código de barras."#,
                p.name
            )
            .into(),
        );
        st.set_lookup_tone(0);
    } else {
        st.set_f_title(format!("Editar: {}", p.name).into());
        st.set_f_badge(p.code.as_deref().filter(|c| !c.is_empty()).map(|c| format!("#{c}")).unwrap_or_else(|| "SEM CÓDIGO".into()).into());
        st.set_f_name(p.name.clone().into());
        st.set_f_code(p.code.clone().unwrap_or_default().into());
        st.set_f_ean(p.ean.clone().unwrap_or_default().into());
    }
    st.set_f_brand(p.brand.clone().unwrap_or_default().into());
    st.set_f_supplier_name(p.supplier_label().unwrap_or("").into());
    st.set_f_supplier_index(supplier_index(p.supplier_id.as_deref()));
    st.set_f_description(p.description.clone().unwrap_or_default().into());
    st.set_f_show_in_menu(p.show_in_menu);
    st.set_f_price(num_text(p.price, 2).into());
    st.set_f_cost(p.cost_price.map(|c| num_text(c, 4)).unwrap_or_default().into());
    let margin = match (p.target_margin, p.cost_price) {
        (Some(m), _) => num_text(m, 1),
        (None, Some(c)) if p.price > 0.0 => num_text((p.price - c) / p.price * 100.0, 1),
        _ => "50".into(),
    };
    st.set_f_margin(margin.into());
    st.set_f_category_index(category_index(&p.category_id));
    st.set_f_kds_index(kds_index(&p.kds_station));
    st.set_f_stock(p.stock.to_string().into());
    st.set_f_min_stock(p.min_stock.to_string().into());
    st.set_f_ncm(p.ncm.clone().unwrap_or_default().into());
    st.set_f_cfop(p.cfop.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "5102".into()).into());
    st.set_f_cest(p.cest.clone().unwrap_or_default().into());
    st.set_f_unit(if p.unit.is_empty() { "un".to_string() } else { p.unit.clone() }.into());
    st.set_f_has_box(p.has_box_price);
    st.set_f_box_qty(p.box_quantity.map(|q| q.to_string()).unwrap_or_else(|| "24".into()).into());
    st.set_f_box_price(p.box_price.map(|b| num_text(b, 2)).unwrap_or_default().into());
    st.set_f_box_ean(p.box_ean.clone().unwrap_or_default().into());
    st.set_f_composed(!d.components.is_empty());
    let (factor, portion) = conversion::suggest_conversion(&p.name, &p.unit, p.has_box_price, p.box_quantity);
    st.set_f_conv_factor(factor.to_string().into());
    st.set_f_conv_portion(portion);
}

/// Abre a edição (ou a cópia) do produto.
pub(crate) fn open_product(ctx: &AppCtx, id: String, duplicate: bool) {
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(
        async move {
            let d = products::detail(&db, &id).await?;
            let image = if duplicate { None } else { digital_menu::product_image(&db, &id).await?.map(|i| i.data) };
            let code = if duplicate { Some(products::next_code(&db, &d.product.category_id).await?) } else { None };
            DbResult::Ok((d, image, code))
        },
        move |ui, result| match result {
            Ok((d, image, code)) => {
                fill_from(ui, &d, duplicate);
                if let Some(code) = code {
                    store(ui).set_f_code(code.into());
                }
                if let Some(img) = image.and_then(|b| image_from(&b, 320)) {
                    store(ui).set_f_photo(img);
                    store(ui).set_f_has_photo(true);
                }
                show_editor(ui);
                if duplicate {
                    with_state(|s| s.form.photo_new = None);
                }
            }
            Err(e) => notify(&next, e.to_string(), true),
        },
    );
}

// ---------- Busca por código de barras ----------

fn lookup(ctx: &AppCtx, mode: LookupMode) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let ean = st.get_f_ean().trim().to_string();
    if ean.is_empty() {
        st.set_lookup_message("Digite ou bipe o Código de Barras (EAN).".into());
        st.set_lookup_tone(2);
        return;
    }
    st.set_lookup_busy(
        match mode {
            LookupMode::All => "all",
            LookupMode::Xml => "xml",
            LookupMode::Global => "global",
        }
        .into(),
    );
    st.set_lookup_message("".into());
    let cat = catalog(ctx);
    ctx.spawn(async move { cat.lookup(&ean, mode).await }, |ui, result| {
        let st = store(ui);
        st.set_lookup_busy("".into());
        let r = match result {
            Ok(r) => r,
            Err(e) => {
                st.set_lookup_message(e.to_string().into());
                st.set_lookup_tone(2);
                return;
            }
        };
        let empty = |v: SharedString| v.trim().is_empty();
        if r.found {
            if let Some(v) = &r.name {
                st.set_f_name(v.into());
            }
            if let Some(v) = r.code.as_ref().filter(|_| empty(st.get_f_code())) {
                st.set_f_code(v.into());
            }
            if !r.ean.is_empty() {
                st.set_f_ean(r.ean.clone().into());
            }
            if let Some(v) = &r.brand {
                st.set_f_brand(v.into());
            }
            if let Some(v) = &r.supplier {
                st.set_f_supplier_name(v.into());
                let low = v.to_lowercase();
                let found = with_state(|s| {
                    s.suppliers
                        .iter()
                        .find(|x| {
                            x.name.to_lowercase().contains(&low)
                                || x.trade_name.as_deref().is_some_and(|t| t.to_lowercase().contains(&low))
                        })
                        .map(|x| x.id.clone())
                });
                if let Some(id) = found {
                    st.set_f_supplier_index(supplier_index(Some(&id)));
                }
            }
            if let Some(v) = &r.ncm {
                st.set_f_ncm(v.into());
            }
            if let Some(v) = &r.cest {
                st.set_f_cest(v.into());
            }
            if let Some(v) = &r.cfop {
                st.set_f_cfop(v.into());
            }
            if let Some(v) = &r.unit {
                st.set_f_unit(v.into());
            }
            if let Some(v) = r.description.as_ref().filter(|_| empty(st.get_f_description())) {
                st.set_f_description(v.into());
            }
            if let Some(v) = &r.kds_station {
                st.set_f_kds_index(kds_index(v));
            }
            if let Some(v) = &r.suggested_category_id {
                st.set_f_category_index(category_index(v));
            }
            if let Some(v) = r.cost_price {
                st.set_f_cost(num_text(v, 4).into());
            }
            if let Some(v) = r.suggested_price.filter(|_| empty(st.get_f_price())) {
                st.set_f_price(num_text(v, 2).into());
            }
            st.set_lookup_tone(1);
            let name = r.name.clone().unwrap_or_default();
            st.set_lookup_message(
                if r.message.is_empty() {
                    format!(r#"Produto "{name}" localizado com sucesso! Tributação e cadastro preenchidos."#)
                } else {
                    r.message.clone()
                }
                .into(),
            );
        } else {
            if let Some(v) = r.code.as_ref().filter(|_| empty(st.get_f_code())) {
                st.set_f_code(v.into());
            }
            if let Some(v) = r.ncm.as_ref().filter(|_| empty(st.get_f_ncm())) {
                st.set_f_ncm(v.into());
            }
            if let Some(v) = r.cfop.as_ref().filter(|_| empty(st.get_f_cfop())) {
                st.set_f_cfop(v.into());
            }
            st.set_lookup_tone(0);
            st.set_lookup_message(
                if r.message.is_empty() { "Código válido. Complete os dados para cadastrar.".into() } else { r.message.clone() }.into(),
            );
        }
        refresh_derived(ui);
    });
}

// ---------- NCM ----------

fn check_ncm(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let clean: String = digits(&st.get_f_ncm()).chars().take(8).collect();
    if clean != st.get_f_ncm().as_str() {
        st.set_f_ncm(clean.clone().into());
    }
    let seq = with_state(|s| {
        s.form.ncm_seq += 1;
        s.form.ncm_seq
    });
    if clean.len() != 8 {
        st.set_ncm_state(0);
        return;
    }
    let Some(server) = ctx.server.clone() else { return };
    st.set_ncm_state(1);
    ctx.spawn(async move { server.fiscal("GET", &format!("/ncm/{clean}"), Corpo::Nada).await }, move |ui, r| {
        if with_state(|s| s.form.ncm_seq) != seq {
            return;
        }
        let st = store(ui);
        let j = r.json();
        if r.ok() && j.get("valido").and_then(|v| v.as_bool()) == Some(true) {
            let desc = j.get("descricao").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ibpt = j.get("aliquotaNacional").and_then(|v| v.as_f64()).map(|a| format!(" · IBPT: {}%", num_text(a, 2))).unwrap_or_default();
            st.set_ncm_detail(format!("{desc}{ibpt}").into());
            st.set_ncm_state(2);
        } else if r.status == 404 || j.get("valido").and_then(|v| v.as_bool()) == Some(false) {
            st.set_ncm_state(3);
        } else {
            st.set_ncm_state(0);
        }
    });
}

// ---------- Foto ----------

/// Escolhe a foto no computador (JPEG, PNG ou WebP).
pub(crate) fn choose_photo_file() -> Option<Vec<u8>> {
    let path = rfd::FileDialog::new()
        .set_title("Foto do produto")
        .add_filter("Imagens (JPEG, PNG, WebP)", &["jpg", "jpeg", "png", "webp"])
        .pick_file()?;
    std::fs::read(path).ok()
}

// ---------- Salvar ----------

fn input_from(ui: &AppWindow) -> Result<ProductInput, String> {
    let st = store(ui);
    let price = parse_num(&st.get_f_price());
    let name = st.get_f_name().trim().to_string();
    let category = with_state(|s| s.form.category_ids.get(st.get_f_category_index().max(0) as usize).cloned());
    let (Some(price), Some(category_id)) = (price.filter(|p| *p >= 0.0), category) else {
        return Err("Preencha os campos obrigatórios (Nome, Preço e Categoria)".into());
    };
    if name.is_empty() {
        return Err("Preencha os campos obrigatórios (Nome, Preço e Categoria)".into());
    }
    let text = |v: SharedString| Some(v.trim().to_string()).filter(|t| !t.is_empty());
    let supplier_id = with_state(|s| {
        let i = st.get_f_supplier_index();
        if i > 0 { s.form.supplier_ids.get(i as usize - 1).cloned() } else { None }
    });
    let composed = st.get_f_composed();
    let components = if composed {
        with_state(|s| {
            s.form
                .components
                .iter()
                .filter_map(|(id, q)| Some((id.clone()?, parse_num(q).unwrap_or(0.0))))
                .collect()
        })
    } else {
        Vec::new()
    };
    let has_box = st.get_f_has_box();
    Ok(ProductInput {
        name,
        code: text(st.get_f_code()).map(|c| c.to_uppercase()),
        ean: text(st.get_f_ean()),
        brand: text(st.get_f_brand()),
        supplier: text(st.get_f_supplier_name()),
        supplier_id,
        description: text(st.get_f_description()),
        show_in_menu: st.get_f_show_in_menu(),
        price,
        cost_price: parse_num(&st.get_f_cost()),
        target_margin: parse_num(&st.get_f_margin()),
        category_id,
        kds_station: kds_of(st.get_f_kds_index()).into(),
        stock: parse_num(&st.get_f_stock()).unwrap_or(0.0),
        min_stock: parse_num(&st.get_f_min_stock()).filter(|v| *v != 0.0).unwrap_or(5.0),
        ncm: text(st.get_f_ncm()),
        cfop: text(st.get_f_cfop()),
        cest: text(st.get_f_cest()),
        unit: text(st.get_f_unit()).map(|u| u.to_lowercase()).unwrap_or_else(|| "un".into()),
        has_box_price: has_box,
        box_quantity: if has_box { Some(st.get_f_box_qty().trim().parse().unwrap_or(24)) } else { None },
        box_price: if has_box { parse_num(&st.get_f_box_price()) } else { None },
        box_ean: if has_box { text(st.get_f_box_ean()) } else { None },
        components,
    })
}

fn save(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let input = match input_from(&ui) {
        Ok(i) => i,
        Err(e) => return notify(ctx, e, true),
    };
    let (editing, groups, groups_changed, photo_new, photo_delete) = with_state(|s| {
        let f = &s.form;
        (
            f.editing.clone(),
            f.group_ids.clone(),
            f.group_ids != f.initial_group_ids,
            f.photo_new.clone(),
            f.photo_removed && f.had_photo,
        )
    });
    let db = ctx.db.clone();
    let user = ctx.user_name();
    let next = ctx.clone();
    ctx.spawn(
        async move {
            let (id, mut events) = match &editing {
                Some(id) => (id.clone(), products::update(&db, id, input, user.as_deref()).await?),
                None => products::create(&db, input, user.as_deref()).await?,
            };
            let mut warnings = Vec::new();
            if groups_changed {
                match modifiers::set_product_groups(&db, &id, &groups).await {
                    Ok(ev) => events.extend(ev),
                    Err(e) => warnings.push(format!("Produto salvo, mas os adicionais não foram gravados: {e}")),
                }
            }
            let photo = match (photo_new, photo_delete) {
                (Some(data), _) => save_product_image(&db, &id, data).await.map(|_| ()),
                (None, true) => digital_menu::delete_product_image(&db, &id).await,
                _ => Ok(()),
            };
            if let Err(e) = photo {
                warnings.push(format!("Produto salvo, mas a foto não foi gravada: {e}"));
            }
            DbResult::Ok((events, warnings))
        },
        move |ui, result| match result {
            Ok((events, warnings)) => {
                emit(&next, &events);
                if warnings.is_empty() {
                    notify(&next, "Produto salvo com sucesso!", false);
                } else {
                    notify(&next, warnings.join(" "), true);
                }
                with_state(|s| s.form = Form::default());
                store(ui).set_screen(CdScreen::Products);
                reload(&next);
            }
            Err(e) => notify(&next, e.to_string(), true),
        },
    );
}

// ---------- Ligações ----------

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);

    st.on_new_product({
        let ctx = ctx.clone();
        move || open_new(&ctx, None)
    });
    st.on_register_barcode({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let ean = store(&ui).get_search().trim().to_string();
            open_new(&ctx, Some(ean));
        }
    });
    st.on_edit_product({
        let ctx = ctx.clone();
        move |id| open_product(&ctx, id.to_string(), false)
    });
    st.on_duplicate_product({
        let ctx = ctx.clone();
        move |id| open_product(&ctx, id.to_string(), true)
    });
    st.on_back({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                with_state(|s| s.form = Form::default());
                store(&ui).set_screen(CdScreen::Products);
            }
        }
    });
    st.on_save({
        let ctx = ctx.clone();
        move || save(&ctx)
    });
    st.on_delete_current({
        let ctx = ctx.clone();
        move || {
            let Some(id) = with_state(|s| s.form.editing.clone()) else { return };
            let Some(ui) = ctx.ui() else { return };
            store(&ui).invoke_delete_product(id.into());
        }
    });
    st.on_lookup({
        let ctx = ctx.clone();
        move |mode| {
            let mode = match mode.as_str() {
                "xml" => LookupMode::Xml,
                "global" => LookupMode::Global,
                _ => LookupMode::All,
            };
            lookup(&ctx, mode);
        }
    });
    st.on_name_edited({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            // Volume em ml/litros no nome: vai para o Bar (e sai da categoria Cozinha)
            if products::has_volume(&st.get_f_name()) {
                st.set_f_kds_index(0);
                let (bar, current) = with_state(|s| {
                    let bar = s.categories.iter().position(|c| c.name.to_lowercase() == "bar");
                    let current = s.categories.get(st.get_f_category_index().max(0) as usize).map(|c| c.name.to_lowercase());
                    (bar, current)
                });
                if let Some(b) = bar {
                    if current.as_deref() == Some("cozinha") || current.is_none() {
                        st.set_f_category_index(b as i32);
                    }
                }
            }
        }
    });
    st.on_category_changed({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let editing = with_state(|s| s.form.editing.is_some());
            if !editing {
                let cat = with_state(|s| s.form.category_ids.get(store(&ui).get_f_category_index().max(0) as usize).cloned());
                if let Some(cat) = cat {
                    load_next_code(&ctx, cat);
                }
            }
        }
    });
    st.on_cost_edited({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let clean = keep_number_chars(&st.get_f_cost());
            if clean != st.get_f_cost().as_str() {
                st.set_f_cost(clean.clone().into());
            }
            let c = parse_num(&clean).unwrap_or(0.0);
            let m = parse_num(&st.get_f_margin()).unwrap_or(0.0);
            if c > 0.0 && m > 0.0 && m < 100.0 {
                st.set_f_price(num_text(round2(c / (1.0 - m / 100.0)), 2).into());
            }
            refresh_derived(&ui);
        }
    });
    st.on_margin_edited({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let clean = keep_number_chars(&st.get_f_margin());
            if clean != st.get_f_margin().as_str() {
                st.set_f_margin(clean.clone().into());
            }
            let m = parse_num(&clean).unwrap_or(0.0);
            let c = parse_num(&st.get_f_cost()).unwrap_or(0.0);
            if c > 0.0 && m > 0.0 && m < 100.0 {
                st.set_f_price(num_text(round2(c / (1.0 - m / 100.0)), 2).into());
            }
            refresh_derived(&ui);
        }
    });
    st.on_price_edited({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let clean = keep_number_chars(&st.get_f_price());
            if clean != st.get_f_price().as_str() {
                st.set_f_price(clean.clone().into());
            }
            let v = parse_num(&clean).unwrap_or(0.0);
            let c = parse_num(&st.get_f_cost()).unwrap_or(0.0);
            if v > 0.0 && c > 0.0 {
                st.set_f_margin(num_text(((v - c) / v * 100.0 * 10.0).round() / 10.0, 1).into());
            }
            refresh_derived(&ui);
        }
    });
    st.on_conv_preset({
        let ctx = ctx.clone();
        move |factor, portion| {
            if let Some(ui) = ctx.ui() {
                store(&ui).set_f_conv_factor(factor);
                store(&ui).set_f_conv_portion(portion);
            }
        }
    });
    st.on_conv_apply({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let factor = st.get_f_conv_factor().trim().parse::<u32>().unwrap_or(1).max(1);
            if factor <= 1 {
                return notify(&ctx, "Informe um fator de conversão maior que 1.", true);
            }
            let cost = parse_num(&st.get_f_cost()).unwrap_or(0.0);
            let stock = parse_num(&st.get_f_stock()).unwrap_or(0.0);
            with_state(|s| {
                s.form.conv_backup = Some(ConvBackup {
                    stock: st.get_f_stock().into(),
                    cost: st.get_f_cost().into(),
                    price: st.get_f_price().into(),
                    factor,
                    has_box: st.get_f_has_box(),
                    box_qty: st.get_f_box_qty().into(),
                    box_price: st.get_f_box_price().into(),
                })
            });
            st.set_f_stock(((stock * factor as f64).round() as i64).to_string().into());
            if cost > 0.0 {
                let unit_cost = (cost / factor as f64 * 10_000.0).round() / 10_000.0;
                st.set_f_cost(num_text(unit_cost, 4).into());
                let m = parse_num(&st.get_f_margin()).filter(|m| *m != 0.0).unwrap_or(50.0);
                if m > 0.0 && m < 100.0 {
                    st.set_f_price(num_text(round2(unit_cost / (1.0 - m / 100.0)), 2).into());
                }
                if st.get_f_box_price().trim().is_empty() {
                    st.set_f_box_price(num_text(round2(cost * 1.5), 2).into());
                }
            }
            st.set_f_has_box(true);
            st.set_f_box_qty(factor.to_string().into());
            st.set_conv_message(
                format!(
                    "✓ Conversão aplicada com sucesso: Estoque foi multiplicado por {factor} e Custo Unitário dividido por {factor}. Clique em Salvar Alterações para gravar."
                )
                .into(),
            );
            refresh_derived(&ui);
        }
    });
    st.on_conv_undo({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let Some(b) = with_state(|s| s.form.conv_backup.take()) else { return };
            let st = store(&ui);
            st.set_f_stock(b.stock.into());
            st.set_f_cost(b.cost.into());
            st.set_f_price(b.price.into());
            st.set_f_has_box(b.has_box);
            st.set_f_box_qty(b.box_qty.into());
            st.set_f_box_price(b.box_price.into());
            st.set_conv_message("".into());
            let _ = b.factor;
            refresh_derived(&ui);
        }
    });
    st.on_use_recipe_cost({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let total = with_state(|s| {
                s.form
                    .components
                    .iter()
                    .filter_map(|(id, q)| {
                        let p = s.products.iter().find(|p| Some(&p.id) == id.as_ref())?;
                        Some(p.cost_price.filter(|c| *c > 0.0).unwrap_or(p.price) * parse_num(q).unwrap_or(0.0))
                    })
                    .sum::<f64>()
            });
            store(&ui).set_f_cost(num_text(round2(total), 2).into());
            store(&ui).invoke_cost_edited();
        }
    });
    st.on_comp_add({
        let ctx = ctx.clone();
        move || {
            with_state(|s| s.form.components.push((None, "1".into())));
            if let Some(ui) = ctx.ui() {
                refresh_derived(&ui);
            }
        }
    });
    st.on_comp_remove({
        let ctx = ctx.clone();
        move |row| {
            with_state(|s| {
                if (row as usize) < s.form.components.len() {
                    s.form.components.remove(row as usize);
                }
            });
            if let Some(ui) = ctx.ui() {
                refresh_derived(&ui);
            }
        }
    });
    st.on_comp_pick({
        let ctx = ctx.clone();
        move |row, index| {
            with_state(|s| {
                let row = row as usize;
                if row >= s.form.components.len() {
                    return;
                }
                let id = if index <= 0 { None } else { s.form.component_ids.get(index as usize - 1).cloned() };
                // Insumo que já estava em outra linha: soma as quantidades e tira esta linha
                if let Some(id) = &id {
                    if let Some(other) = s.form.components.iter().enumerate().position(|(i, c)| i != row && c.0.as_ref() == Some(id)) {
                        let cur = parse_num(&s.form.components[row].1).unwrap_or(1.0);
                        let exist = parse_num(&s.form.components[other].1).unwrap_or(0.0);
                        s.form.components[other].1 = num_text(((exist + cur) * 1000.0).round() / 1000.0, 3);
                        s.form.components.remove(row);
                        return;
                    }
                }
                s.form.components[row].0 = id;
            });
            if let Some(ui) = ctx.ui() {
                store(&ui).set_f_components(model(Vec::new()));
                refresh_derived(&ui);
            }
        }
    });
    st.on_comp_step({
        let ctx = ctx.clone();
        move |row, dir| {
            with_state(|s| {
                if let Some(c) = s.form.components.get_mut(row as usize) {
                    let cur = parse_num(&c.1).unwrap_or(0.0);
                    let next = if dir < 0 { (cur - if cur > 1.0 { 1.0 } else { 0.1 }).max(0.01) } else { cur + if cur >= 1.0 { 1.0 } else { 0.1 } };
                    c.1 = num_text((next * 1000.0).round() / 1000.0, 3);
                }
            });
            if let Some(ui) = ctx.ui() {
                refresh_derived(&ui);
            }
        }
    });
    st.on_comp_qty({
        let ctx = ctx.clone();
        move |row, text| {
            with_state(|s| {
                if let Some(c) = s.form.components.get_mut(row as usize) {
                    c.1 = keep_number_chars(&text);
                }
            });
            if let Some(ui) = ctx.ui() {
                refresh_derived(&ui);
            }
        }
    });
    st.on_group_toggle({
        let ctx = ctx.clone();
        move |id| {
            with_state(|s| {
                let id = id.to_string();
                if let Some(i) = s.form.group_ids.iter().position(|g| g == &id) {
                    s.form.group_ids.remove(i);
                } else {
                    s.form.group_ids.push(id);
                }
            });
            if let Some(ui) = ctx.ui() {
                render_groups(&ui);
            }
        }
    });
    st.on_pick_photo({
        let ctx = ctx.clone();
        move || {
            let Some(data) = choose_photo_file() else { return };
            ctx.spawn(async move { photo::shrink_photo(&data) }, |ui, result| match result {
                Ok(jpeg) => {
                    if let Some(img) = image_from(&jpeg, 320) {
                        store(ui).set_f_photo(img);
                        store(ui).set_f_has_photo(true);
                    }
                    with_state(|s| {
                        s.form.photo_new = Some(jpeg);
                        s.form.photo_removed = false;
                    });
                }
                Err(e) => tracing::warn!("{e}"),
            });
        }
    });
    st.on_remove_photo({
        let ctx = ctx.clone();
        move || {
            with_state(|s| {
                s.form.photo_new = None;
                s.form.photo_removed = true;
            });
            if let Some(ui) = ctx.ui() {
                store(&ui).set_f_has_photo(false);
                store(&ui).set_f_photo(Image::default());
            }
        }
    });
    st.on_supplier_changed({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let i = st.get_f_supplier_index();
            let name = with_state(|s| {
                let id = if i > 0 { s.form.supplier_ids.get(i as usize - 1)? } else { return None };
                let sup = s.suppliers.iter().find(|x| &x.id == id)?;
                Some(sup.trade_name.clone().filter(|t| !t.is_empty()).unwrap_or(sup.name.clone()))
            });
            st.set_f_supplier_name(name.unwrap_or_default().into());
            refresh_derived(&ui);
        }
    });
    st.on_open_whatsapp({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                let phone = digits(&store(&ui).get_sup_card_phone());
                if !phone.is_empty() {
                    open_external(&format!("https://wa.me/55{phone}"));
                }
            }
        }
    });
    st.on_open_quick_supplier({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                let st = store(&ui);
                st.set_q_name("".into());
                st.set_q_trade("".into());
                st.set_q_doc("".into());
                st.set_q_phone("".into());
                st.set_modal_error("".into());
                st.set_modal(CdModal::QuickSupplier);
            }
        }
    });
    st.on_q_save({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let name = st.get_q_name().trim().to_string();
            if name.is_empty() {
                st.set_modal_error("Informe a Razão Social ou Nome do fornecedor".into());
                return;
            }
            let text = |v: SharedString| Some(v.trim().to_string()).filter(|t| !t.is_empty());
            let input = suppliers::SupplierInput {
                name: Some(name),
                trade_name: text(st.get_q_trade()),
                document: text(st.get_q_doc()),
                phone: text(st.get_q_phone()),
                ..Default::default()
            };
            let db = ctx.db.clone();
            ctx.spawn(
                async move {
                    let created = suppliers::create(&db, input).await?;
                    let all = suppliers::list(&db, None).await?;
                    DbResult::Ok((created, all))
                },
                |ui, result| match result {
                    Ok((created, all)) => {
                        with_state(|s: &mut State| s.suppliers = all);
                        fill_choices(ui);
                        let st = store(ui);
                        st.set_f_supplier_index(supplier_index(Some(&created.id)));
                        st.set_f_supplier_name(created.trade_name.clone().unwrap_or(created.name.clone()).into());
                        st.set_modal(CdModal::None);
                        refresh_derived(ui);
                    }
                    Err(e) => store(ui).set_modal_error(e.to_string().into()),
                },
            );
        }
    });
    st.on_ncm_edited({
        let ctx = ctx.clone();
        move || check_ncm(&ctx)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kds() {
        assert_eq!((kds_index("KITCHEN"), kds_index("NONE"), kds_index("BAR")), (1, 2, 0));
        assert_eq!((kds_of(1), kds_of(2), kds_of(0)), ("KITCHEN", "NONE", "BAR"));
        assert_eq!(num_fixed(60.0, 1), "60,0");
    }
}
