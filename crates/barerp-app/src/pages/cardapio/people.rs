//! Fornecedores (`SuppliersView.tsx` + "Gestão de Fornecedores" do ProductsView) e clientes / fiado
//! (`CustomersView.tsx`).

use slint::SharedString;

use barerp_core::money::brl;
use barerp_db::customers::{self, NewCustomer};
use barerp_db::suppliers::{self, Supplier, SupplierInput};
use barerp_server::Corpo;

use super::{ask, digits, format_document, notify, parse_num, reload, store, with_state, Confirm};
use crate::ctx::{open_external, AppCtx};
use crate::view::model;
use crate::{AppWindow, CdCustomer, CdModal, CdSupplier};

#[derive(Default)]
pub(crate) struct PeopleState {
    editing_supplier: Option<String>,
    paying_customer: Option<String>,
}

// ---------- Fornecedores ----------

fn supplier_view(s: &Supplier) -> CdSupplier {
    let trade = s.trade_name.clone().filter(|t| !t.is_empty());
    CdSupplier {
        id: s.id.clone().into(),
        title: trade.clone().unwrap_or(s.name.clone()).into(),
        razao: if trade.is_some() { format!("({})", s.name) } else { String::new() }.into(),
        document: s.document.as_deref().map(format_document).unwrap_or_default().into(),
        products: if s.product_count == 1 { "1 produto vinculado".to_string() } else { format!("{} produtos vinculados", s.product_count) }.into(),
        phone: s.phone.clone().unwrap_or_default().into(),
        email: s.email.clone().unwrap_or_default().into(),
        contact: s.contact_name.clone().unwrap_or_default().into(),
        city: match (&s.city, &s.state) {
            (Some(c), Some(uf)) if !c.is_empty() && !uf.is_empty() => format!("{c} - {uf}"),
            (Some(c), _) => c.clone(),
            _ => String::new(),
        }
        .into(),
        notes: s.notes.clone().unwrap_or_default().into(),
    }
}

/// Lista filtrada e indicadores (total, com documento, produtos vinculados).
pub(crate) fn render_suppliers(ui: &AppWindow) {
    let st = store(ui);
    let term = st.get_sup_search().trim().to_lowercase();
    let (rows, total, with_doc, linked) = with_state(|s| {
        let has = |v: &Option<String>| v.as_deref().is_some_and(|t| t.to_lowercase().contains(&term));
        let rows: Vec<CdSupplier> = s
            .suppliers
            .iter()
            .filter(|x| term.is_empty() || x.name.to_lowercase().contains(&term) || has(&x.trade_name) || has(&x.document) || has(&x.contact_name) || has(&x.city) || has(&x.phone))
            .map(supplier_view)
            .collect();
        let with_doc = s.suppliers.iter().filter(|x| x.document.as_deref().is_some_and(|d| d.len() >= 11)).count();
        let linked: i64 = s.suppliers.iter().map(|x| x.product_count).sum();
        (rows, s.suppliers.len(), with_doc, linked)
    });
    st.set_suppliers(model(rows));
    st.set_sup_all(total as i32);
    st.set_sup_total(total.to_string().into());
    st.set_sup_with_doc(with_doc.to_string().into());
    st.set_sup_linked(linked.to_string().into());
}

pub(crate) fn open_suppliers(ctx: &AppCtx) {
    if let Some(ui) = ctx.ui() {
        render_suppliers(&ui);
    }
    reload(ctx);
}

fn open_supplier_form(ui: &AppWindow, s: Option<&Supplier>) {
    with_state(|st| st.people.editing_supplier = s.map(|x| x.id.clone()));
    let st = store(ui);
    let v = |o: Option<&Option<String>>| o.and_then(|x| x.clone()).unwrap_or_default();
    st.set_sf_editing(s.is_some());
    st.set_sf_name(s.map(|x| x.name.clone()).unwrap_or_default().into());
    st.set_sf_trade(v(s.map(|x| &x.trade_name)).into());
    st.set_sf_doc(v(s.map(|x| &x.document)).into());
    st.set_sf_ie(v(s.map(|x| &x.ie)).into());
    st.set_sf_phone(v(s.map(|x| &x.phone)).into());
    st.set_sf_email(v(s.map(|x| &x.email)).into());
    st.set_sf_city(v(s.map(|x| &x.city)).into());
    st.set_sf_uf(v(s.map(|x| &x.state)).into());
    st.set_sf_address(v(s.map(|x| &x.address)).into());
    st.set_sf_contact(v(s.map(|x| &x.contact_name)).into());
    st.set_sf_notes(v(s.map(|x| &x.notes)).into());
    st.set_sf_cnpj_busy(false);
    st.set_modal_error("".into());
    st.set_modal(CdModal::SupplierForm);
}

fn save_supplier(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    if st.get_sf_name().trim().is_empty() {
        st.set_modal_error("Razão Social / Nome do fornecedor é obrigatório".into());
        return;
    }
    // Na alteração, campo vazio apaga (Some("")); na criação, vazio fica sem valor
    let t = |v: SharedString| Some(v.trim().to_string());
    let input = SupplierInput {
        name: t(st.get_sf_name()),
        trade_name: t(st.get_sf_trade()),
        document: t(st.get_sf_doc()),
        ie: t(st.get_sf_ie()),
        phone: t(st.get_sf_phone()),
        email: t(st.get_sf_email()),
        city: t(st.get_sf_city()),
        state: t(st.get_sf_uf()).map(|u| u.to_uppercase()),
        address: t(st.get_sf_address()),
        contact_name: t(st.get_sf_contact()),
        notes: t(st.get_sf_notes()),
    };
    let editing = with_state(|s| s.people.editing_supplier.clone());
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(
        async move {
            match editing {
                Some(id) => suppliers::update(&db, &id, input).await,
                None => suppliers::create(&db, input).await,
            }
        },
        move |ui, result| match result {
            Ok(_) => {
                store(ui).set_modal(CdModal::None);
                reload(&next);
            }
            Err(e) => store(ui).set_modal_error(e.to_string().into()),
        },
    );
}

/// Preenche os campos vazios com o cadastro da Receita (BrasilAPI), como o "Consultar CNPJ" do original.
fn lookup_cnpj(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    let clean = digits(&st.get_sf_doc());
    if clean.len() != 14 {
        st.set_modal_error("Digite um CNPJ com 14 dígitos para consultar.".into());
        return;
    }
    let Some(server) = ctx.server.clone() else {
        st.set_modal_error("Consulta de CNPJ indisponível: o servidor interno não está rodando.".into());
        return;
    };
    st.set_sf_cnpj_busy(true);
    st.set_modal_error("".into());
    ctx.spawn(async move { server.fiscal("GET", &format!("/consulta-cnpj/{clean}"), Corpo::Nada).await }, |ui, r| {
        let st = store(ui);
        st.set_sf_cnpj_busy(false);
        let j = r.json();
        let txt = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if !r.ok() || txt("razaoSocial").is_empty() {
            let e = r.erro();
            st.set_modal_error(if e.trim().is_empty() { "CNPJ não encontrado na Receita Federal.".into() } else { e }.into());
            return;
        }
        let fill = |current: SharedString, value: String| if current.trim().is_empty() && !value.is_empty() { Some(value) } else { None };
        if let Some(v) = fill(st.get_sf_name(), txt("razaoSocial")) {
            st.set_sf_name(v.into());
        }
        if let Some(v) = fill(st.get_sf_trade(), txt("nomeFantasia")) {
            st.set_sf_trade(v.into());
        }
        if let Some(v) = fill(st.get_sf_phone(), txt("telefone")) {
            st.set_sf_phone(v.into());
        }
        if let Some(v) = fill(st.get_sf_city(), txt("municipio")) {
            st.set_sf_city(v.into());
        }
        if let Some(v) = fill(st.get_sf_uf(), txt("uf")) {
            st.set_sf_uf(v.into());
        }
        if let Some(v) = fill(st.get_sf_ie(), txt("ie")) {
            st.set_sf_ie(v.into());
        }
        let street = txt("logradouro");
        if !street.is_empty() {
            let mut addr = street;
            if !txt("numero").is_empty() {
                addr.push_str(&format!(", {}", txt("numero")));
            }
            if !txt("bairro").is_empty() {
                addr.push_str(&format!(" - {}", txt("bairro")));
            }
            if !txt("cep").is_empty() {
                addr.push_str(&format!(" (CEP {})", txt("cep")));
            }
            if let Some(v) = fill(st.get_sf_address(), addr) {
                st.set_sf_address(v.into());
            }
        }
    });
}

pub(crate) fn delete_supplier(ctx: &AppCtx, id: String) {
    let db = ctx.db.clone();
    let next = ctx.clone();
    ctx.spawn(async move { suppliers::delete(&db, &id).await }, move |_, result| match result {
        Ok(()) => {
            notify(&next, "Fornecedor removido com sucesso", false);
            reload(&next);
        }
        Err(e) => notify(&next, e.to_string(), true),
    });
}

// ---------- Clientes ----------

fn render_customers(ui: &AppWindow) {
    let st = store(ui);
    let term = st.get_cu_search().trim().to_lowercase();
    let rows: Vec<CdCustomer> = with_state(|s| {
        s.customers
            .iter()
            .filter(|c| term.is_empty() || c.name.to_lowercase().contains(&term))
            .map(|c| CdCustomer {
                id: c.id.clone().into(),
                name: c.name.clone().into(),
                phone: c.phone.clone().unwrap_or_default().into(),
                document: c.document.clone().unwrap_or_default().into(),
                debt: brl(c.credit_tab_balance).into(),
                owes: c.credit_tab_balance > 0.0,
            })
            .collect()
    });
    st.set_customers(model(rows));
}

pub(crate) fn open_customers(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    store(&ui).set_cu_loading(true);
    let db = ctx.db.clone();
    ctx.spawn(async move { customers::list(&db).await }, |ui, result| {
        store(ui).set_cu_loading(false);
        if let Ok(list) = result {
            with_state(|s| s.customers = list);
        }
        render_customers(ui);
    });
}

pub(crate) fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let st = store(&ui);
    st.on_sup_search_changed({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render_suppliers(&ui);
            }
        }
    });
    st.on_sup_new({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                open_supplier_form(&ui, None);
            }
        }
    });
    st.on_sup_edit({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            if let Some(s) = with_state(|s| s.suppliers.iter().find(|x| x.id == id.as_str()).cloned()) {
                open_supplier_form(&ui, Some(&s));
            }
        }
    });
    st.on_sup_delete({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let name = with_state(|s| {
                s.suppliers.iter().find(|x| x.id == id.as_str()).map(|x| x.trade_name.clone().filter(|t| !t.is_empty()).unwrap_or(x.name.clone()))
            })
            .unwrap_or_default();
            ask(
                &ui,
                "Excluir fornecedor",
                &format!("Deseja realmente excluir o fornecedor \"{name}\"?\nOs produtos continuarão no cardápio."),
                "Excluir",
                true,
                Confirm::DeleteSupplier(id.to_string()),
            );
        }
    });
    st.on_sup_whatsapp({
        move |id| {
            let phone = with_state(|s| s.suppliers.iter().find(|x| x.id == id.as_str()).and_then(|x| x.phone.clone())).unwrap_or_default();
            let d = digits(&phone);
            if !d.is_empty() {
                open_external(&format!("https://wa.me/55{d}"));
            }
        }
    });
    st.on_sf_save({
        let ctx = ctx.clone();
        move || save_supplier(&ctx)
    });
    st.on_sf_cnpj({
        let ctx = ctx.clone();
        move || lookup_cnpj(&ctx)
    });

    st.on_cu_search_changed({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                render_customers(&ui);
            }
        }
    });
    st.on_cu_new({
        let ctx = ctx.clone();
        move || {
            if let Some(ui) = ctx.ui() {
                let st = store(&ui);
                st.set_cf_name("".into());
                st.set_cf_phone("".into());
                st.set_cf_doc("".into());
                st.set_modal_error("".into());
                st.set_modal(CdModal::CustomerForm);
            }
        }
    });
    st.on_cf_save({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            if st.get_cf_name().trim().is_empty() {
                return;
            }
            let opt = |v: SharedString| Some(v.trim().to_string()).filter(|t| !t.is_empty());
            let input = NewCustomer { name: st.get_cf_name().to_string(), phone: opt(st.get_cf_phone()), document: opt(st.get_cf_doc()), notes: None };
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(async move { customers::create(&db, input).await }, move |ui, result| match result {
                Ok(_) => {
                    store(ui).set_modal(CdModal::None);
                    open_customers(&next);
                }
                Err(e) => store(ui).set_modal_error(format!("Erro ao criar cliente: {e}").into()),
            });
        }
    });
    st.on_cu_pay({
        let ctx = ctx.clone();
        move |id| {
            let Some(ui) = ctx.ui() else { return };
            let Some(c) = with_state(|s| s.customers.iter().find(|c| c.id == id.as_str()).cloned()) else { return };
            with_state(|s| s.people.paying_customer = Some(c.id.clone()));
            let st = store(&ui);
            st.set_pd_name(c.name.clone().into());
            st.set_pd_debt(brl(c.credit_tab_balance).into());
            st.set_pd_amount("".into());
            st.set_modal_error("".into());
            st.set_modal(CdModal::PayDebt);
        }
    });
    st.on_pd_save({
        let ctx = ctx.clone();
        move || {
            let Some(ui) = ctx.ui() else { return };
            let st = store(&ui);
            let Some(amount) = parse_num(&st.get_pd_amount()).filter(|a| *a > 0.0) else {
                st.set_modal_error("Informe um valor válido maior que zero.".into());
                return;
            };
            let Some(id) = with_state(|s| s.people.paying_customer.clone()) else { return };
            let db = ctx.db.clone();
            let next = ctx.clone();
            ctx.spawn(
                async move { customers::pay_credit_tab(&db, &id, amount).await },
                move |ui, result| match result {
                    Ok(left) => {
                        store(ui).set_modal(CdModal::None);
                        notify(&next, format!("Pagamento de fiado registrado. Saldo restante: {}", brl(left)), false);
                        open_customers(&next);
                    }
                    Err(e) => store(ui).set_modal_error(e.to_string().into()),
                },
            );
        }
    });
}
