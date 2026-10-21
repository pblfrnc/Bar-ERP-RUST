//! Entrada (dados do cadastro) e saída (cardápio publicado, mesmo formato de `menu/src/types.ts`).
//!
//! A ordem dos campos das structs de saída é a ordem do objeto montado no Node: o hash do JSON
//! depende dela.

use std::collections::HashMap;

use serde::{Serialize, Serializer};

/// Número como o `JSON.stringify` escreve: inteiro sem `.0` (`10`, não `10.0`).
pub(crate) fn js_number<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 {
        // -0 vira 0, como no JS
        s.serialize_i64(*v as i64)
    } else if v.is_finite() {
        s.serialize_f64(*v)
    } else {
        s.serialize_none()
    }
}

// ---------- Entrada ----------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceCategory {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceOption {
    pub name: String,
    pub price: f64,
    pub is_active: bool,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceModifierGroup {
    pub name: String,
    pub is_active: bool,
    pub min_select: i64,
    pub max_select: i64,
    pub options: Vec<SourceOption>,
}

/// Ligação produto ↔ grupo de adicionais (`ProductModifierGroup`), com a ordem no produto.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceGroupLink {
    pub sort_order: i64,
    pub group: SourceModifierGroup,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceProduct {
    pub id: String,
    pub category_id: String,
    pub name: String,
    pub description: Option<String>,
    pub price: f64,
    pub is_active: bool,
    pub show_in_menu: bool,
    /// BAR, KITCHEN ou NONE
    pub kds_station: String,
    pub track_stock: bool,
    pub stock: i64,
    pub cost_price: Option<f64>,
    pub has_box_price: bool,
    pub box_price: Option<f64>,
    pub box_quantity: Option<i64>,
    pub box_cost_price: Option<f64>,
    pub modifier_groups: Vec<SourceGroupLink>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourcePromotion {
    pub id: String,
    pub name: String,
    pub is_active: bool,
    pub days_of_week: String,
    pub start_time: String,
    pub end_time: String,
    pub discount_type: String,
    pub value: f64,
    pub product_ids: String,
    pub category_ids: String,
}

/// Tudo o que o cardápio usa do banco. Pode vir sem filtro nem ordem: [`crate::build_menu`] aplica
/// os mesmos `where`/`orderBy` das consultas do Node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuSource {
    pub categories: Vec<SourceCategory>,
    pub products: Vec<SourceProduct>,
    pub promotions: Vec<SourcePromotion>,
    /// productId → hash da foto (`ProductImage.hash`)
    pub image_hashes: HashMap<String, String>,
    /// Nome do bar quando o título do cardápio está vazio ([`crate::default_title`]).
    pub default_title: String,
    /// Minutos como o `Date.getTimezoneOffset()` (Brasília = 180); [`crate::js_timezone_offset`].
    pub timezone_offset: i32,
}

// ---------- Saída ----------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MenuOption {
    pub name: String,
    #[serde(serialize_with = "js_number")]
    pub price: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MenuGroup {
    pub name: String,
    pub min: i64,
    /// 0 = sem limite
    pub max: i64,
    pub options: Vec<MenuOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MenuBox {
    pub quantity: i64,
    #[serde(serialize_with = "js_number")]
    pub price: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuProduct {
    pub id: String,
    pub category_id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(serialize_with = "js_number")]
    pub price: f64,
    pub sold_out: bool,
    /// Caminho no próprio site (`/api/menu/<loja>/img/<hash>`)
    pub image: Option<String>,
    #[serde(rename = "box")]
    pub box_: Option<MenuBox>,
    pub groups: Vec<MenuGroup>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MenuKind {
    Food,
    Drink,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MenuCategory {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// Comidas vêm antes e com mais destaque no site
    pub kind: MenuKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuPromotion {
    pub id: String,
    pub name: String,
    pub is_active: bool,
    pub days_of_week: String,
    pub start_time: String,
    pub end_time: String,
    pub discount_type: String,
    #[serde(serialize_with = "js_number")]
    pub value: f64,
    pub product_ids: String,
    pub category_ids: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuStoreInfo {
    pub name: String,
    pub message: Option<String>,
    /// O site calcula o horário das promoções no fuso do PC do bar
    pub timezone_offset: i32,
}

/// Cardápio publicado (sem `updatedAt`, que entra só no envio e não no hash).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublishedMenu {
    pub version: u32,
    pub store: MenuStoreInfo,
    pub categories: Vec<MenuCategory>,
    pub products: Vec<MenuProduct>,
    pub promotions: Vec<MenuPromotion>,
}

/// Corpo do PUT: `{ ...menu, products, updatedAt }` (products fica na posição original).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishBody<'a> {
    pub version: u32,
    pub store: &'a MenuStoreInfo,
    pub categories: &'a [MenuCategory],
    pub products: &'a [MenuProduct],
    pub promotions: &'a [MenuPromotion],
    pub updated_at: String,
}
