//! `/api/products`: leituras (cardápio do celular, categorias, adicionais, promoções do dia e fotos); as gravações
//! ficam em `products_admin.rs`.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use barerp_core::pricing::{is_promotion_active_at, Promotion};
use chrono::{Local, Offset, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiResult};
use crate::prisma::{self, str_field, Obj};
use crate::AppState;

pub fn router() -> Router<AppState> {
    use super::products_admin as admin;
    let router = Router::new()
        .route("/", get(list).post(admin::create_product))
        .route("/:id", put(admin::update_product).delete(admin::delete_product))
        .route("/categories/all", get(categories))
        .route("/modifier-groups", get(modifier_groups).post(admin::create_group))
        .route("/promotions", get(promotions).post(admin::create_promotion))
        .route("/promotions/active", get(active_promotions))
        .route("/:id/image", get(image).put(admin::put_image).delete(admin::delete_image));
    admin::routes(router)
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(rename = "categoryId")]
    category_id: Option<String>,
    search: Option<String>,
    #[serde(rename = "activeOnly")]
    active_only: Option<String>,
}

fn by_id(rows: Vec<Obj>) -> HashMap<String, Obj> {
    rows.into_iter().filter_map(|r| Some((str_field(&r, "id")?.to_string(), r))).collect()
}

fn group_by(rows: Vec<Obj>, key: &str) -> HashMap<String, Vec<Obj>> {
    let mut map: HashMap<String, Vec<Obj>> = HashMap::new();
    for row in rows {
        if let Some(k) = str_field(&row, key).map(String::from) {
            map.entry(k).or_default().push(row);
        }
    }
    map
}

fn obj_or_null(obj: Option<&Obj>) -> Value {
    obj.cloned().map(Value::Object).unwrap_or(Value::Null)
}

/// Produtos com `category`, `supplierRel`, `components[].component`, `modifierGroups[].group.options`
/// (só opções ativas; grupos inativos vêm junto para o cadastro não perder o vínculo) e `imageHash`.
async fn list(State(state): State<AppState>, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let mut filter = vec!["1 = 1".to_string()];
    let mut binds: Vec<String> = Vec::new();
    if q.active_only.as_deref() != Some("false") {
        filter.push(r#"p."isActive" = 1"#.into());
    }
    if let Some(cat) = q.category_id.filter(|c| !c.is_empty()) {
        filter.push(r#"p."categoryId" = ?"#.into());
        binds.push(cat);
    }
    if let Some(s) = q.search.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        // `contains` do Prisma no SQLite é LIKE (sem diferença de maiúsculas em letras sem acento)
        let like = format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let cols = [r#"p."name""#, r#"p."code""#, r#"p."ean""#, r#"p."brand""#, r#"p."supplier""#, r#"s."name""#, r#"s."tradeName""#];
        filter.push(format!("({})", cols.map(|c| format!(r"{c} LIKE ? ESCAPE '\'")).join(" OR ")));
        binds.extend(std::iter::repeat_n(like, cols.len()));
    }
    let sql = format!(
        r#"SELECT p.* FROM "Product" p LEFT JOIN "Supplier" s ON s."id" = p."supplierId"
           WHERE {} ORDER BY p."categoryId" ASC, p."name" ASC"#,
        filter.join(" AND ")
    );
    let mut query = sqlx::query::<sqlx::Sqlite>(&sql);
    for b in &binds {
        query = query.bind(b.clone());
    }
    let mut conn = state.db.pool().acquire().await?;
    let products: Vec<Obj> = query.fetch_all(&mut *conn).await?.iter().map(prisma::row_to_obj).collect();
    if products.is_empty() {
        return Ok(Json(json!([])));
    }

    let categories = by_id(prisma::select(&mut conn, "Category", "", &[]).await?);
    let suppliers = by_id(prisma::select(&mut conn, "Supplier", "", &[]).await?);
    let all_products = by_id(prisma::select(&mut conn, "Product", "", &[]).await?);
    let components = group_by(prisma::select(&mut conn, "ProductComponent", "ORDER BY rowid", &[]).await?, "parentProductId");
    let links = group_by(
        prisma::select(&mut conn, "ProductModifierGroup", r#"ORDER BY "sortOrder" ASC"#, &[]).await?,
        "productId",
    );
    let groups = by_id(prisma::select(&mut conn, "ModifierGroup", "", &[]).await?);
    let options = group_by(
        prisma::select(&mut conn, "ModifierOption", r#"WHERE "isActive" = 1 ORDER BY "sortOrder" ASC"#, &[]).await?,
        "groupId",
    );
    let hashes: HashMap<String, String> = sqlx::query_as(r#"SELECT "productId", "hash" FROM "ProductImage""#)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect();

    let list: Vec<Value> = products
        .into_iter()
        .map(|mut p| {
            let id = str_field(&p, "id").unwrap_or_default().to_string();
            let category = obj_or_null(str_field(&p, "categoryId").and_then(|c| categories.get(c)));
            let supplier = obj_or_null(str_field(&p, "supplierId").and_then(|s| suppliers.get(s)));
            let comps: Vec<Value> = components
                .get(&id)
                .into_iter()
                .flatten()
                .map(|c| {
                    let mut c = c.clone();
                    let component = obj_or_null(str_field(&c, "componentId").and_then(|cid| all_products.get(cid)));
                    c.insert("component".into(), component);
                    Value::Object(c)
                })
                .collect();
            let mods: Vec<Value> = links
                .get(&id)
                .into_iter()
                .flatten()
                .map(|link| {
                    let mut link = link.clone();
                    let group = str_field(&link, "groupId").and_then(|g| groups.get(g)).cloned().map(|mut g| {
                        let gid = str_field(&g, "id").unwrap_or_default().to_string();
                        let opts = options.get(&gid).cloned().unwrap_or_default();
                        g.insert("options".into(), Value::Array(opts.into_iter().map(Value::Object).collect()));
                        Value::Object(g)
                    });
                    link.insert("group".into(), group.unwrap_or(Value::Null));
                    Value::Object(link)
                })
                .collect();
            p.insert("category".into(), category);
            p.insert("supplierRel".into(), supplier);
            p.insert("components".into(), Value::Array(comps));
            p.insert("modifierGroups".into(), Value::Array(mods));
            p.insert("imageHash".into(), hashes.get(&id).cloned().map(Value::String).unwrap_or(Value::Null));
            Value::Object(p)
        })
        .collect();
    Ok(Json(Value::Array(list)))
}

/// Categorias por `sortOrder` com `_count.products`.
async fn categories(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mut conn = state.db.pool().acquire().await?;
    let mut list = prisma::select(&mut conn, "Category", r#"ORDER BY "sortOrder" ASC"#, &[]).await?;
    let counts: HashMap<String, i64> =
        sqlx::query_as(r#"SELECT "categoryId", COUNT(*) FROM "Product" GROUP BY "categoryId""#)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    for cat in list.iter_mut() {
        let n = str_field(cat, "id").and_then(|id| counts.get(id)).copied().unwrap_or(0);
        cat.insert("_count".into(), json!({ "products": n }));
    }
    Ok(Json(json!(list)))
}

/// Grupos de adicionais com todas as opções e `_count.products`.
async fn modifier_groups(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mut conn = state.db.pool().acquire().await?;
    let mut list = prisma::select(&mut conn, "ModifierGroup", r#"ORDER BY "sortOrder" ASC, "name" ASC"#, &[]).await?;
    let mut options = group_by(prisma::select(&mut conn, "ModifierOption", r#"ORDER BY "sortOrder" ASC"#, &[]).await?, "groupId");
    let counts: HashMap<String, i64> =
        sqlx::query_as(r#"SELECT "groupId", COUNT(*) FROM "ProductModifierGroup" GROUP BY "groupId""#)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    for group in list.iter_mut() {
        let id = str_field(group, "id").unwrap_or_default().to_string();
        let opts = options.remove(&id).unwrap_or_default();
        group.insert("options".into(), json!(opts));
        group.insert("_count".into(), json!({ "products": counts.get(&id).copied().unwrap_or(0) }));
    }
    Ok(Json(json!(list)))
}

fn promotion_rule(p: &Obj) -> Promotion {
    let s = |k: &str| str_field(p, k).unwrap_or_default().to_string();
    Promotion {
        id: s("id"),
        name: s("name"),
        is_active: p.get("isActive").and_then(Value::as_bool).unwrap_or(false),
        days_of_week: s("daysOfWeek"),
        start_time: s("startTime"),
        end_time: s("endTime"),
        discount_type: s("discountType"),
        value: p.get("value").and_then(Value::as_f64).unwrap_or(0.0),
        product_ids: s("productIds"),
        category_ids: s("categoryIds"),
    }
}

async fn promotions(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mut conn = state.db.pool().acquire().await?;
    let mut list = prisma::select(&mut conn, "Promotion", r#"ORDER BY "isActive" DESC, "name" ASC"#, &[]).await?;
    let now = Local::now().naive_local();
    for promo in list.iter_mut() {
        let running = is_promotion_active_at(&promotion_rule(promo), now);
        promo.insert("isRunningNow".into(), json!(running));
    }
    Ok(Json(json!(list)))
}

/// Promoções ligadas e o relógio do PC: o celular calcula o horário no fuso do servidor.
async fn active_promotions(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let mut conn = state.db.pool().acquire().await?;
    let list = prisma::select(&mut conn, "Promotion", r#"WHERE "isActive" = 1 ORDER BY rowid"#, &[]).await?;
    // `Date.getTimezoneOffset()`: minutos de UTC menos a hora local (Brasília = 180)
    let offset = -Local::now().offset().fix().local_minus_utc() / 60;
    Ok(Json(json!({
        "serverTime": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "serverTimezoneOffset": offset,
        "promotions": list,
    })))
}

async fn image(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    let row: Option<(Vec<u8>, String)> =
        sqlx::query_as(r#"SELECT "data", "mimeType" FROM "ProductImage" WHERE "productId" = ?"#)
            .bind(&id)
            .fetch_optional(state.db.pool())
            .await
            .map_err(|_| ApiError::internal("Erro ao ler a foto do produto"))?;
    let (data, mime) = row.ok_or_else(|| ApiError::not_found("Produto sem foto"))?;
    let mime = HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("image/jpeg"));
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            // A URL leva ?v=<hash>, então pode ficar em cache; o <img> do celular é de outra origem
            (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable")),
            (header::HeaderName::from_static("cross-origin-resource-policy"), HeaderValue::from_static("cross-origin")),
        ],
        data,
    )
        .into_response())
}
