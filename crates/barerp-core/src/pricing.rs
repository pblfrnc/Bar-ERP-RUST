//! Preço da linha do pedido: promoções do dia (happy hour) e adicionais/perguntas.
//! Tradução de `server/src/services/pricing.ts` — o preço que vale é sempre o calculado aqui.

use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Promotion {
    pub id: String,
    pub name: String,
    pub is_active: bool,
    /// "0,1,2,3,4,5,6" — 0 = domingo
    pub days_of_week: String,
    /// HH:MM
    pub start_time: String,
    /// HH:MM — menor que o início atravessa a meia-noite
    pub end_time: String,
    /// PERCENT ou FIXED_PRICE
    pub discount_type: String,
    pub value: f64,
    /// JSON com ids de produtos
    pub product_ids: String,
    /// JSON com ids de categorias
    pub category_ids: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModifierSnapshot {
    pub group: String,
    pub name: String,
    pub price: f64,
}

#[derive(Debug, Clone)]
pub struct ModifierOption {
    pub id: String,
    pub name: String,
    pub price: f64,
}

/// Grupo de adicionais ligado ao produto, já filtrado (grupo e opções ativos) e na ordem do produto.
#[derive(Debug, Clone)]
pub struct ModifierGroup {
    pub name: String,
    pub min_select: i64,
    pub max_select: i64,
    pub options: Vec<ModifierOption>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct PricingError(pub String);

/// Arredonda como o `Math.round(v * 100) / 100` do JavaScript (meio para cima).
pub fn round2(v: f64) -> f64 {
    (v * 100.0 + 0.5).floor() / 100.0
}

fn to_minutes(hhmm: &str) -> u32 {
    let mut parts = hhmm.split(':').map(|n| n.trim().parse::<u32>().unwrap_or(0));
    parts.next().unwrap_or(0) * 60 + parts.next().unwrap_or(0)
}

fn parse_id_list(json: &str) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::Array(list)) => list
            .into_iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Observação que a cozinha vê: opções escolhidas + observação digitada.
pub fn kitchen_note(modifiers: Option<&str>, notes: Option<&str>) -> Option<String> {
    let mods = modifiers
        .and_then(|m| serde_json::from_str::<Vec<serde_json::Value>>(m).ok())
        .map(|list| {
            list.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let notes = notes.unwrap_or("").trim();
    match (mods.is_empty(), notes.is_empty()) {
        (false, false) => Some(format!("{mods} | {notes}")),
        (false, true) => Some(mods),
        (true, false) => Some(notes.to_string()),
        (true, true) => None,
    }
}

pub fn is_promotion_active_at(promo: &Promotion, now: NaiveDateTime) -> bool {
    if !promo.is_active {
        return false;
    }
    let days: Vec<u32> = promo
        .days_of_week
        .split(',')
        .filter_map(|d| d.trim().parse().ok())
        .collect();
    let today = now.weekday().num_days_from_sunday();
    let minutes = now.hour() * 60 + now.minute();
    let start = to_minutes(&promo.start_time);
    let end = to_minutes(&promo.end_time);

    if start <= end {
        return days.contains(&today) && minutes >= start && minutes <= end;
    }
    // Atravessa a meia-noite (ex: 18:00 às 02:00): a madrugada pertence ao dia em que a promoção começou
    if minutes >= start {
        return days.contains(&today);
    }
    if minutes <= end {
        return days.contains(&((today + 6) % 7));
    }
    false
}

pub fn promotion_applies_to(promo: &Promotion, product_id: &str, category_id: &str) -> bool {
    parse_id_list(&promo.product_ids).iter().any(|id| id == product_id)
        || parse_id_list(&promo.category_ids).iter().any(|id| id == category_id)
}

/// Menor preço entre as promoções ativas para o produto (preço de unidade, nunca caixa fechada).
pub fn apply_best_promotion<'a>(
    base_price: f64,
    product_id: &str,
    category_id: &str,
    promotions: &'a [Promotion],
    now: NaiveDateTime,
) -> (f64, Option<&'a Promotion>) {
    let mut best = (base_price, None);
    for promo in promotions {
        if !is_promotion_active_at(promo, now) || !promotion_applies_to(promo, product_id, category_id) {
            continue;
        }
        let price = if promo.discount_type == "FIXED_PRICE" {
            promo.value
        } else {
            base_price * (1.0 - promo.value.clamp(0.0, 100.0) / 100.0)
        };
        if price >= 0.0 && price < best.0 {
            best = (round2(price), Some(promo));
        }
    }
    best
}

/// Valida as opções escolhidas contra os grupos do produto e devolve o acréscimo + o que foi escolhido.
/// `option_ids == None` significa app antigo (não sabe escolher opções).
pub fn resolve_modifiers(
    product_name: &str,
    groups: &[ModifierGroup],
    option_ids: Option<&[String]>,
) -> Result<(f64, Vec<ModifierSnapshot>), PricingError> {
    let mut selected: Vec<&str> = Vec::new();
    for id in option_ids.unwrap_or(&[]) {
        if !selected.contains(&id.as_str()) {
            selected.push(id);
        }
    }

    if groups.is_empty() {
        if !selected.is_empty() {
            return Err(PricingError(format!("\"{product_name}\" não tem adicionais cadastrados.")));
        }
        return Ok((0.0, Vec::new()));
    }

    if option_ids.is_none() && groups.iter().any(|g| g.min_select > 0) {
        return Err(PricingError(format!(
            "\"{product_name}\" tem perguntas obrigatórias. Atualize o app do garçom para escolher as opções."
        )));
    }

    let mut snapshot = Vec::new();
    let mut extra = 0.0;
    let mut used: Vec<&str> = Vec::new();

    for group in groups {
        let chosen: Vec<&ModifierOption> =
            group.options.iter().filter(|o| selected.contains(&o.id.as_str())).collect();
        used.extend(chosen.iter().map(|o| o.id.as_str()));
        let count = chosen.len() as i64;
        if count < group.min_select {
            return Err(PricingError(if group.min_select == 1 {
                format!("Escolha \"{}\" para {product_name}.", group.name)
            } else {
                format!(
                    "Escolha pelo menos {} opções em \"{}\" para {product_name}.",
                    group.min_select, group.name
                )
            }));
        }
        if group.max_select > 0 && count > group.max_select {
            return Err(PricingError(format!(
                "No máximo {} opção(ões) em \"{}\" para {product_name}.",
                group.max_select, group.name
            )));
        }
        for o in chosen {
            snapshot.push(ModifierSnapshot { group: group.name.clone(), name: o.name.clone(), price: o.price });
            extra += o.price;
        }
    }

    if selected.iter().any(|id| !used.contains(id)) {
        return Err(PricingError(format!(
            "Uma das opções escolhidas para \"{product_name}\" não existe mais. Atualize o cardápio e tente de novo."
        )));
    }

    Ok((round2(extra), snapshot))
}

#[derive(Debug, Clone)]
pub struct ProductForPricing<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub category_id: &'a str,
    pub price: f64,
    pub box_price: Option<f64>,
    pub has_box_price: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricedLine {
    pub is_box: bool,
    pub unit_price: f64,
    pub base_price: Option<f64>,
    pub promotion_name: Option<String>,
    /// JSON gravado em `OrderItem.modifiers`
    pub modifiers: Option<String>,
}

/// Preço final de uma linha do pedido. Caixa fechada não recebe promoção nem adicionais.
pub fn price_order_line(
    product: &ProductForPricing,
    unit_type: Option<&str>,
    groups: &[ModifierGroup],
    option_ids: Option<&[String]>,
    promotions: &[Promotion],
    now: NaiveDateTime,
) -> Result<PricedLine, PricingError> {
    let box_price = product.box_price.filter(|p| *p != 0.0);
    if unit_type == Some("BOX") && product.has_box_price {
        if let Some(box_price) = box_price {
            return Ok(PricedLine {
                is_box: true,
                unit_price: box_price,
                base_price: None,
                promotion_name: None,
                modifiers: None,
            });
        }
    }

    let (extra, snapshot) = resolve_modifiers(product.name, groups, option_ids)?;
    let (price, promo) = apply_best_promotion(product.price, product.id, product.category_id, promotions, now);
    Ok(PricedLine {
        is_box: false,
        unit_price: round2(price + extra),
        base_price: promo.map(|_| round2(product.price + extra)),
        promotion_name: promo.map(|p| p.name.clone()),
        modifiers: (!snapshot.is_empty())
            .then(|| serde_json::to_string(&snapshot).expect("snapshot sempre serializa")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()
    }

    fn promo(days: &str, start: &str, end: &str, kind: &str, value: f64) -> Promotion {
        Promotion {
            id: "p1".into(),
            name: "Happy Hour".into(),
            is_active: true,
            days_of_week: days.into(),
            start_time: start.into(),
            end_time: end.into(),
            discount_type: kind.into(),
            value,
            product_ids: r#"["chopp"]"#.into(),
            category_ids: "[]".into(),
        }
    }

    #[test]
    fn crosses_midnight_belongs_to_start_day() {
        // 17/10/2026 é sábado (6); 18/10/2026 é domingo (0)
        let p = promo("6", "18:00", "02:00", "PERCENT", 50.0);
        assert!(is_promotion_active_at(&p, at(2026, 10, 17, 19, 0)));
        assert!(is_promotion_active_at(&p, at(2026, 10, 18, 1, 30)));
        assert!(!is_promotion_active_at(&p, at(2026, 10, 18, 19, 0)));
        assert!(!is_promotion_active_at(&p, at(2026, 10, 17, 3, 0)));
    }

    #[test]
    fn best_promotion_and_rounding() {
        let promos = vec![
            promo("0,1,2,3,4,5,6", "00:00", "23:59", "PERCENT", 33.0),
            promo("0,1,2,3,4,5,6", "00:00", "23:59", "FIXED_PRICE", 9.0),
        ];
        let (price, p) = apply_best_promotion(12.9, "chopp", "bar", &promos, at(2026, 10, 18, 12, 0));
        assert_eq!(price, 8.64);
        assert_eq!(p.unwrap().discount_type, "PERCENT");
        let (price, p) = apply_best_promotion(12.9, "outro", "bar", &promos, at(2026, 10, 18, 12, 0));
        assert_eq!((price, p.is_none()), (12.9, true));
    }

    #[test]
    fn modifiers_rules() {
        let groups = vec![ModifierGroup {
            name: "Ponto da carne".into(),
            min_select: 1,
            max_select: 1,
            options: vec![
                ModifierOption { id: "mal".into(), name: "Mal passada".into(), price: 0.0 },
                ModifierOption { id: "bacon".into(), name: "Bacon".into(), price: 4.5 },
            ],
        }];
        assert!(resolve_modifiers("Picanha", &groups, None).unwrap_err().0.contains("Atualize o app"));
        assert_eq!(resolve_modifiers("Picanha", &groups, Some(&[])).unwrap_err().0, "Escolha \"Ponto da carne\" para Picanha.");
        let both = ["mal".to_string(), "bacon".to_string()];
        assert!(resolve_modifiers("Picanha", &groups, Some(&both)).unwrap_err().0.starts_with("No máximo 1"));
        let ghost = ["sumiu".to_string()];
        assert!(resolve_modifiers("Picanha", &[], Some(&ghost)).unwrap_err().0.contains("não tem adicionais"));
        let (extra, snap) = resolve_modifiers("Picanha", &groups, Some(&["bacon".to_string()])).unwrap();
        assert_eq!((extra, snap.len()), (4.5, 1));
    }

    #[test]
    fn box_line_ignores_promotion_and_modifiers() {
        let product = ProductForPricing {
            id: "chopp",
            name: "Chopp",
            category_id: "bar",
            price: 10.0,
            box_price: Some(200.0),
            has_box_price: true,
        };
        let promos = vec![promo("0,1,2,3,4,5,6", "00:00", "23:59", "PERCENT", 50.0)];
        let now = at(2026, 10, 18, 12, 0);
        let line = price_order_line(&product, Some("BOX"), &[], None, &promos, now).unwrap();
        assert_eq!((line.is_box, line.unit_price), (true, 200.0));
        let line = price_order_line(&product, None, &[], None, &promos, now).unwrap();
        assert_eq!(line.unit_price, 5.0);
        assert_eq!(line.base_price, Some(10.0));
        assert_eq!(line.promotion_name.as_deref(), Some("Happy Hour"));
    }

    #[test]
    fn kitchen_note_format() {
        let mods = r#"[{"group":"Ponto","name":"Mal passada","price":0}]"#;
        assert_eq!(kitchen_note(Some(mods), Some(" sem sal ")).as_deref(), Some("Mal passada | sem sal"));
        assert_eq!(kitchen_note(None, Some("  ")), None);
        assert_eq!(kitchen_note(Some("lixo"), Some("gelo")).as_deref(), Some("gelo"));
    }
}
