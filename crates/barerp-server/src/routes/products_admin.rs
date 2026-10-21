//! `/api/products` (gravações): cadastro de produtos, categorias, adicionais, promoções, fotos e ferramentas do
//! catálogo. Tradução da parte de escrita de `server/src/routes/products.ts` (o modo admin do APK usa; no PC a
//! tela é a área Cardápio). As regras ficam em `barerp_db::{products, modifiers, promotions}`; cada gravação
//! emite `catalog:updated`, e o app nativo agenda a publicação do cardápio digital (como o `scheduleMenuPublish`).

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use axum::routing::{get, post, put};
use serde::Deserialize;
use serde_json::{json, Value};

use barerp_catalog::{Catalog, LookupMode};
use barerp_db::events::Event;
use barerp_db::modifiers::{self, GroupInput, OptionInput};
use barerp_db::products::{self, ProductInput};
use barerp_db::promotions::{self, PromotionInput};
use barerp_db::{digital_menu, DbError};

use crate::error::{ApiError, ApiResult};
use crate::events::emit_all;
use crate::http::{js_number, js_truthy, Body};
use crate::prisma;
use crate::AppState;

/// Rotas de gravação, somadas ao roteador de leitura em `products.rs` (as que dividem o caminho com uma leitura,
/// como `/`, `/:id/image`, `/modifier-groups` e `/promotions`, ficam lá).
pub fn routes(router: Router<AppState>) -> Router<AppState> {
    router
        .route("/menu-visibility", put(menu_visibility))
        .route("/organize-bar-kitchen", post(organize))
        .route("/backfill-codes", post(backfill_codes))
        .route("/fix-duplicate-codes", post(fix_duplicate_codes))
        .route("/lookup-ean/:ean", get(lookup_ean))
        .route("/stock-report", get(stock_report))
        .route("/categories", post(create_category))
        .route("/categories/:id", put(update_category).delete(delete_category))
        .route("/categories/:id/next-code", get(next_code))
        .route("/categories/:id/resequence", post(resequence))
        .route("/modifier-groups/:id", put(update_group).delete(delete_group))
        .route("/promotions/:id", put(update_promotion).delete(delete_promotion))
        .route("/:id/price-history", get(price_history))
        .route("/:id/modifier-groups", put(product_groups))
}

async fn emit(state: &AppState, events: &[Event]) {
    emit_all(state, events).await;
}

/// Produto no formato do Prisma (com `category`).
async fn product_json(state: &AppState, id: &str) -> ApiResult<Value> {
    let mut conn = state.db.pool().acquire().await.map_err(DbError::from)?;
    let mut product = prisma::select_one(&mut conn, "Product", id)
        .await?
        .ok_or_else(|| ApiError::not_found("Produto não encontrado"))?;
    if let Some(cat) = prisma::str_field(&product, "categoryId").map(str::to_string) {
        let category = prisma::select_one(&mut conn, "Category", &cat).await?;
        product.insert("category".into(), category.map(Value::Object).unwrap_or(Value::Null));
    }
    Ok(Value::Object(product))
}

// ---------- Produto ----------

/// Texto opcional do corpo: vazio vira `None`.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Número opcional (`null`/"" = sem valor), como os `!== '' ? Number(x) : null` do Node.
fn opt_num(v: &Value) -> Option<f64> {
    match v {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        other => js_number(other),
    }
}

fn components(v: &Value) -> Option<Vec<(String, f64)>> {
    v.as_array().map(|list| {
        list.iter()
            .filter_map(|c| Some((c.get("componentId")?.as_str()?.to_string(), c.get("quantity").and_then(js_number)?)))
            .collect()
    })
}

/// Aplica no formulário só os campos que vieram no corpo (o PUT do Node é parcial).
fn apply(input: &mut ProductInput, body: &Body) {
    let b = &body.0;
    if let Some(v) = b.get("name") {
        input.name = v.as_str().unwrap_or_default().trim().to_string();
    }
    if let Some(v) = b.get("code") {
        input.code = text(v);
    }
    if let Some(v) = b.get("ean") {
        input.ean = text(v);
    }
    if let Some(v) = b.get("supplier") {
        input.supplier = text(v);
    }
    if let Some(v) = b.get("supplierId") {
        input.supplier_id = text(v);
    }
    if let Some(v) = b.get("brand") {
        input.brand = text(v);
    }
    if let Some(v) = b.get("ncm") {
        input.ncm = text(v);
    }
    if let Some(v) = b.get("cfop") {
        input.cfop = text(v);
    }
    if let Some(v) = b.get("cest") {
        input.cest = text(v);
    }
    if let Some(v) = b.get("unit") {
        input.unit = text(v).unwrap_or_else(|| "un".into());
    }
    if let Some(v) = b.get("description") {
        input.description = text(v);
    }
    if let Some(v) = b.get("price").and_then(js_number) {
        input.price = v;
    }
    if let Some(v) = b.get("costPrice") {
        input.cost_price = opt_num(v);
    }
    if let Some(v) = b.get("targetMargin") {
        input.target_margin = opt_num(v);
    }
    if let Some(v) = b.get("hasBoxPrice") {
        input.has_box_price = js_truthy(v);
    }
    if let Some(v) = b.get("boxQuantity") {
        input.box_quantity = Some(js_number(v).filter(|n| *n != 0.0).map(|n| n as i64).unwrap_or(24));
    }
    if let Some(v) = b.get("boxPrice") {
        input.box_price = opt_num(v);
    }
    if let Some(v) = b.get("boxEan") {
        input.box_ean = text(v);
    }
    if let Some(v) = b.get("categoryId").and_then(Value::as_str) {
        input.category_id = v.to_string();
    }
    if let Some(v) = b.get("kdsStation").and_then(Value::as_str) {
        input.kds_station = v.to_string();
    }
    if let Some(v) = b.get("stock").and_then(js_number) {
        input.stock = v;
    }
    if let Some(v) = b.get("minStock").and_then(js_number) {
        input.min_stock = v;
    }
    if let Some(v) = b.get("showInMenu") {
        input.show_in_menu = js_truthy(v);
    }
    if let Some(list) = b.get("components").and_then(components) {
        input.components = list;
    }
}

fn changed_by(body: &Body) -> Option<String> {
    body.str("changedBy")
}

pub(crate) async fn create_product(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    if body.str("name").is_none() || body.get("price").is_none() || body.str("categoryId").is_none() {
        return Err(ApiError::bad_request("Nome, preço e categoria são obrigatórios"));
    }
    let mut input = ProductInput {
        unit: "un".into(),
        kds_station: "BAR".into(),
        stock: 100.0,
        min_stock: 10.0,
        show_in_menu: true,
        box_quantity: Some(24),
        ..Default::default()
    };
    apply(&mut input, &body);
    let (id, events) = products::create(&state.db, input, changed_by(&body).as_deref()).await?;
    emit(&state, &events).await;
    Ok((StatusCode::CREATED, Json(product_json(&state, &id).await?)))
}

pub(crate) async fn update_product(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let d = products::detail(&state.db, &id).await?;
    let p = d.product;
    let mut input = ProductInput {
        name: p.name,
        code: p.code,
        ean: p.ean,
        brand: p.brand,
        supplier: p.supplier,
        supplier_id: p.supplier_id,
        description: p.description,
        show_in_menu: p.show_in_menu,
        price: p.price,
        cost_price: p.cost_price,
        target_margin: p.target_margin,
        category_id: p.category_id,
        kds_station: p.kds_station,
        stock: p.stock as f64,
        min_stock: p.min_stock as f64,
        ncm: p.ncm,
        cfop: p.cfop,
        cest: p.cest,
        unit: p.unit,
        has_box_price: p.has_box_price,
        box_quantity: p.box_quantity,
        box_price: p.box_price,
        box_ean: p.box_ean,
        components: d.components.into_iter().map(|c| (c.component_id, c.quantity)).collect(),
    };
    apply(&mut input, &body);
    let events = products::update(&state.db, &id, input, changed_by(&body).as_deref()).await?;
    emit(&state, &events).await;
    if let Some(active) = body.bool("isActive") {
        let events = products::set_active(&state.db, &id, active).await?;
        emit(&state, &events).await;
    }
    Ok(Json(product_json(&state, &id).await?))
}

pub(crate) async fn delete_product(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (message, events) = products::delete(&state.db, &id).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "success": true, "message": message })))
}

async fn price_history(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(products::price_history(&state.db, &id).await?)))
}

async fn stock_report(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(products::stock_report(&state.db).await?)))
}

#[derive(Deserialize)]
struct LookupQuery {
    mode: Option<String>,
}

async fn lookup_ean(State(state): State<AppState>, Path(ean): Path<String>, Query(q): Query<LookupQuery>) -> ApiResult<Json<Value>> {
    let mode = match q.mode.as_deref() {
        Some("xml") => LookupMode::Xml,
        Some("global") => LookupMode::Global,
        _ => LookupMode::All,
    };
    Ok(Json(json!(Catalog::new(state.db.clone()).lookup(&ean, mode).await?)))
}

// ---------- Fotos e cardápio digital ----------

async fn menu_visibility(State(state): State<AppState>, body: Body) -> ApiResult<Json<Value>> {
    let ids = |k: &str| -> Vec<String> {
        body.array(k).map(|l| l.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect()).unwrap_or_default()
    };
    digital_menu::set_menu_visibility(&state.db, &ids("show"), &ids("hide")).await?;
    emit(&state, &[Event::CatalogUpdated { scope: "products" }]).await;
    Ok(Json(json!({ "success": true })))
}

pub(crate) async fn put_image(State(state): State<AppState>, Path(id): Path<String>, data: Bytes) -> ApiResult<Json<Value>> {
    if data.is_empty() {
        return Err(ApiError::bad_request("Envie a foto (JPEG, PNG ou WebP)."));
    }
    if digital_menu::detect_image_mime(&data).is_none() {
        return Err(ApiError::bad_request("Formato de imagem não suportado. Use JPEG, PNG ou WebP."));
    }
    products::detail(&state.db, &id).await?;
    let hash = barerp_menu_publish::db_store::save_product_image(&state.db, &id, data.to_vec()).await?;
    emit(&state, &[Event::CatalogUpdated { scope: "products" }]).await;
    Ok(Json(json!({ "imageHash": hash })))
}

pub(crate) async fn delete_image(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    digital_menu::delete_product_image(&state.db, &id).await?;
    emit(&state, &[Event::CatalogUpdated { scope: "products" }]).await;
    Ok(Json(json!({ "success": true })))
}

// ---------- Ferramentas do catálogo ----------

async fn organize(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (message, events) = products::organize_bar_kitchen(&state.db).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "ok": true, "message": message })))
}

async fn backfill_codes(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (message, events) = products::backfill_codes(&state.db).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "ok": true, "mensagem": message })))
}

async fn fix_duplicate_codes(State(state): State<AppState>, body: Body) -> ApiResult<Json<Value>> {
    let all = body.str("mode").as_deref() == Some("resequence_all");
    let (message, events) = products::fix_duplicate_codes(&state.db, all).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "ok": true, "mensagem": message })))
}

// ---------- Categorias ----------

async fn category_json(state: &AppState, id: &str) -> ApiResult<Value> {
    let mut conn = state.db.pool().acquire().await.map_err(DbError::from)?;
    let cat = prisma::select_one(&mut conn, "Category", id).await?.ok_or_else(|| ApiError::not_found("Categoria não encontrada"))?;
    Ok(Value::Object(cat))
}

async fn create_category(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = body.str("name").unwrap_or_default();
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("Nome da categoria é obrigatório"));
    }
    let code_start = body.num("codeStart").filter(|n| *n > 0.0).map(|n| n as i64);
    let (id, events) = products::create_category(&state.db, &name, body.str("icon").as_deref(), code_start).await?;
    emit(&state, &events).await;
    Ok((StatusCode::CREATED, Json(category_json(&state, &id).await?)))
}

async fn update_category(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let current = products::categories(&state.db)
        .await?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ApiError::not_found("Categoria não encontrada"))?;
    let name = body.str("name").unwrap_or(current.name);
    let code_start = body.num("codeStart").map(|n| n as i64);
    let (_, events) = products::update_category(&state.db, &id, &name, body.str("icon").as_deref(), code_start, false).await?;
    emit(&state, &events).await;
    Ok(Json(category_json(&state, &id).await?))
}

async fn delete_category(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (message, events) = products::delete_category(&state.db, &id).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "success": true, "message": message })))
}

async fn next_code(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let code = products::next_code(&state.db, &id).await?;
    Ok(Json(json!({ "categoryId": id, "nextCode": code })))
}

async fn resequence(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (message, events) = products::resequence_category(&state.db, &id).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "ok": true, "mensagem": message })))
}

// ---------- Adicionais ----------

/// `parseModifierGroupBody` do Node (a validação fica no `barerp_db::modifiers`).
fn group_input(body: &Body) -> GroupInput {
    let int = |k: &str| body.get(k).and_then(js_number).map(|n| n.trunc() as i64).unwrap_or(0).max(0);
    let options = body
        .array("options")
        .map(|list| {
            list.iter()
                .map(|o| OptionInput {
                    id: o.get("id").and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty()),
                    name: o.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                    price: o
                        .get("price")
                        .map(|p| match p {
                            Value::String(s) => s.replace(',', ".").trim().parse::<f64>().unwrap_or(0.0),
                            other => js_number(other).unwrap_or(0.0),
                        })
                        .unwrap_or(0.0)
                        .max(0.0),
                    is_active: o.get("isActive") != Some(&Value::Bool(false)),
                })
                .collect()
        })
        .unwrap_or_default();
    GroupInput {
        name: body.str("name").unwrap_or_default().trim().to_string(),
        min_select: int("minSelect"),
        max_select: int("maxSelect"),
        is_active: body.get("isActive") != Some(&Value::Bool(false)),
        options,
    }
}

async fn group_json(state: &AppState, id: &str) -> ApiResult<Value> {
    let group = modifiers::list(&state.db).await?.into_iter().find(|g| g.id == id);
    Ok(json!(group))
}

pub(crate) async fn create_group(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    let (id, events) = modifiers::save(&state.db, None, group_input(&body)).await?;
    emit(&state, &events).await;
    Ok((StatusCode::CREATED, Json(group_json(&state, &id).await?)))
}

async fn update_group(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let (id, events) = modifiers::save(&state.db, Some(&id), group_input(&body)).await?;
    emit(&state, &events).await;
    Ok(Json(group_json(&state, &id).await?))
}

async fn delete_group(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let events = modifiers::delete(&state.db, &id).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "success": true })))
}

async fn product_groups(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let mut ids: Vec<String> = Vec::new();
    for v in body.array("groupIds").cloned().unwrap_or_default() {
        let s = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
        if !ids.contains(&s) {
            ids.push(s);
        }
    }
    products::detail(&state.db, &id).await?;
    let events = modifiers::set_product_groups(&state.db, &id, &ids).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "success": true })))
}

// ---------- Promoções ----------

/// `parsePromotionBody` do Node (a validação fica no `barerp_db::promotions`).
fn promotion_input(body: &Body) -> PromotionInput {
    let days: Vec<Value> = match body.get("daysOfWeek") {
        Some(Value::Array(list)) => list.clone(),
        Some(Value::String(s)) => s.split(',').map(|d| Value::String(d.to_string())).collect(),
        _ => Vec::new(),
    };
    let mut days: Vec<u32> = days
        .iter()
        .filter_map(|d| match d {
            Value::String(s) => s.trim().parse::<u32>().ok(),
            other => js_number(other).map(|n| n as u32),
        })
        .filter(|d| *d <= 6)
        .collect();
    days.sort_unstable();
    days.dedup();
    let list = |k: &str| -> Vec<String> {
        body.array(k).map(|l| l.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect()).unwrap_or_default()
    };
    PromotionInput {
        name: body.str("name").unwrap_or_default().trim().to_string(),
        is_active: body.get("isActive") != Some(&Value::Bool(false)),
        days,
        start_time: body.str("startTime").unwrap_or_else(|| "00:00".into()),
        end_time: body.str("endTime").unwrap_or_else(|| "23:59".into()),
        fixed_price: body.str("discountType").as_deref() == Some("FIXED_PRICE"),
        value: body
            .get("value")
            .map(|v| match v {
                Value::String(s) => s.replace(',', ".").trim().parse::<f64>().unwrap_or(f64::NAN),
                other => js_number(other).unwrap_or(f64::NAN),
            })
            .unwrap_or(f64::NAN),
        product_ids: list("productIds"),
        category_ids: list("categoryIds"),
    }
}

async fn promotion_json(state: &AppState, id: &str) -> ApiResult<Value> {
    let promo = promotions::list(&state.db).await?.into_iter().find(|p| p.id == id);
    Ok(json!(promo))
}

pub(crate) async fn create_promotion(State(state): State<AppState>, body: Body) -> ApiResult<(StatusCode, Json<Value>)> {
    let (id, events) = promotions::save(&state.db, None, promotion_input(&body)).await?;
    emit(&state, &events).await;
    Ok((StatusCode::CREATED, Json(promotion_json(&state, &id).await?)))
}

async fn update_promotion(State(state): State<AppState>, Path(id): Path<String>, body: Body) -> ApiResult<Json<Value>> {
    let (id, events) = promotions::save(&state.db, Some(&id), promotion_input(&body)).await?;
    emit(&state, &events).await;
    Ok(Json(promotion_json(&state, &id).await?))
}

async fn delete_promotion(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let events = promotions::delete(&state.db, &id).await?;
    emit(&state, &events).await;
    Ok(Json(json!({ "success": true })))
}
