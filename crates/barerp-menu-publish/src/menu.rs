//! Montagem do cardápio publicado (`buildMenu` do Node) e hashes.

use std::collections::{HashMap, HashSet};

use chrono::{Local, Offset};
use sha2::{Digest, Sha256};

use crate::model::*;
use crate::settings::{parse_ids, DigitalMenuSettings};

/// Nome do bar para o título: nome fantasia (fiscal), senão nome do restaurante, senão "Cardápio".
pub fn default_title(nome_fantasia: Option<&str>, restaurant_name: Option<&str>) -> String {
    [nome_fantasia, restaurant_name]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("Cardápio")
        .to_string()
}

/// `new Date().getTimezoneOffset()`: minutos de UTC menos a hora local (Brasília = 180).
pub fn js_timezone_offset() -> i32 {
    -Local::now().offset().fix().local_minus_utc() / 60
}

/// Hash da foto que vai na URL: SHA-256 em hex, 32 primeiros caracteres (o site confere).
pub fn image_hash(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))[..32].to_string()
}

/// `sha256(JSON.stringify(menu))` em hex — igual ao Node, para não republicar à toa depois da troca.
pub fn menu_hash(menu: &PublishedMenu) -> String {
    let json = serde_json::to_string(menu).expect("cardápio sempre serializa");
    hex::encode(Sha256::digest(json.as_bytes()))
}

fn trimmed(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Caixa fechada só aparece com preço acima do custo (preço de caixa igual ao custo é engano no cadastro/importação).
fn menu_box(p: &SourceProduct) -> Option<MenuBox> {
    let quantity = p.box_quantity.unwrap_or(0);
    let price = p.box_price.unwrap_or(0.0);
    if !p.has_box_price || price <= 0.0 || quantity <= 0 {
        return None;
    }
    let box_cost = match p.box_cost_price.filter(|v| *v != 0.0) {
        Some(c) => c,
        None => p
            .cost_price
            .filter(|v| *v != 0.0)
            .map_or(0.0, |c| c * quantity as f64),
    };
    if box_cost > 0.0 && price <= box_cost + 0.005 {
        return None;
    }
    Some(MenuBox { quantity, price })
}

/// Monta o cardápio (mesmo formato de `menu/src/types.ts`). Nada de custo, estoque ou dados fiscais.
pub fn build_menu(settings: &DigitalMenuSettings, source: &MenuSource) -> PublishedMenu {
    let hidden: HashSet<String> = parse_ids(&settings.hidden_category_ids)
        .into_iter()
        .collect();
    let store_id = settings.store_id.as_deref().unwrap_or("null");

    // Mesmas consultas do Node: produtos ativos, no cardápio e com preço, por nome
    let mut products: Vec<&SourceProduct> = source
        .products
        .iter()
        .filter(|p| p.is_active && p.show_in_menu && p.price > 0.0)
        .collect();
    products.sort_by(|a, b| a.name.cmp(&b.name));

    let mut menu_products: Vec<(MenuProduct, bool)> = Vec::new();
    for p in products
        .into_iter()
        .filter(|p| !hidden.contains(&p.category_id))
    {
        let kitchen = p.kds_station == "KITCHEN";
        // Comida da cozinha é feita na hora: estoque zerado não quer dizer que acabou
        let sold_out = !settings.ignore_stock && !kitchen && p.track_stock && p.stock <= 0;
        if settings.hide_sold_out && sold_out {
            continue;
        }
        let mut links: Vec<&SourceGroupLink> = p.modifier_groups.iter().collect();
        links.sort_by_key(|l| l.sort_order);
        let groups = links
            .into_iter()
            .map(|l| &l.group)
            .filter_map(|g| {
                let mut options: Vec<&SourceOption> =
                    g.options.iter().filter(|o| o.is_active).collect();
                options.sort_by_key(|o| o.sort_order);
                (g.is_active && !options.is_empty()).then(|| MenuGroup {
                    name: g.name.clone(),
                    min: g.min_select,
                    max: g.max_select,
                    options: options
                        .into_iter()
                        .map(|o| MenuOption {
                            name: o.name.clone(),
                            price: o.price,
                        })
                        .collect(),
                })
            })
            .collect();
        menu_products.push((
            MenuProduct {
                id: p.id.clone(),
                category_id: p.category_id.clone(),
                name: p.name.clone(),
                description: trimmed(&p.description),
                price: p.price,
                sold_out,
                image: source
                    .image_hashes
                    .get(&p.id)
                    .map(|h| format!("/api/menu/{store_id}/img/{h}")),
                box_: menu_box(p),
                groups,
            },
            kitchen,
        ));
    }

    // Categoria é "comida" quando a maioria dos itens sai na cozinha; comidas vêm antes das bebidas no site
    let mut count: HashMap<&str, (usize, usize)> = HashMap::new();
    for (p, kitchen) in &menu_products {
        let c = count.entry(p.category_id.as_str()).or_default();
        c.1 += 1;
        if *kitchen {
            c.0 += 1;
        }
    }
    let kind_of = |id: &str| match count.get(id) {
        Some((k, total)) if k * 2 > *total => MenuKind::Food,
        _ => MenuKind::Drink,
    };
    let mut cats: Vec<&SourceCategory> = source.categories.iter().collect();
    cats.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut categories: Vec<MenuCategory> = cats
        .into_iter()
        .filter(|c| count.contains_key(c.id.as_str()))
        .map(|c| MenuCategory {
            id: c.id.clone(),
            name: c.name.clone(),
            icon: c.icon.clone().filter(|i| !i.is_empty()),
            kind: kind_of(&c.id),
        })
        .collect();
    // sort estável: mantém a ordem do cadastro dentro de cada grupo
    categories.sort_by_key(|c| c.kind != MenuKind::Food);

    let mut promotions: Vec<&SourcePromotion> =
        source.promotions.iter().filter(|p| p.is_active).collect();
    promotions.sort_by(|a, b| a.name.cmp(&b.name));

    PublishedMenu {
        version: 1,
        store: MenuStoreInfo {
            name: trimmed(&settings.title).unwrap_or_else(|| source.default_title.clone()),
            message: trimmed(&settings.message),
            timezone_offset: source.timezone_offset,
        },
        categories,
        products: menu_products.into_iter().map(|(p, _)| p).collect(),
        promotions: promotions
            .into_iter()
            .map(|p| MenuPromotion {
                id: p.id.clone(),
                name: p.name.clone(),
                is_active: p.is_active,
                days_of_week: p.days_of_week.clone(),
                start_time: p.start_time.clone(),
                end_time: p.end_time.clone(),
                discount_type: p.discount_type.clone(),
                value: p.value,
                product_ids: p.product_ids.clone(),
                category_ids: p.category_ids.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titulo_padrao() {
        assert_eq!(default_title(Some("  "), Some(" Bar do Zé ")), "Bar do Zé");
        assert_eq!(default_title(Some("Zé Bebidas"), Some("Bar")), "Zé Bebidas");
        assert_eq!(default_title(None, None), "Cardápio");
    }

    #[test]
    fn hash_da_foto() {
        // sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(image_hash(b"abc"), "ba7816bf8f01cfea414140de5dae2223");
    }

    #[test]
    fn caixa_so_acima_do_custo() {
        let mut p = SourceProduct {
            has_box_price: true,
            box_price: Some(60.0),
            box_quantity: Some(12),
            cost_price: Some(5.0),
            ..Default::default()
        };
        assert_eq!(
            menu_box(&p),
            None,
            "12 x 5,00 = 60,00: preço igual ao custo"
        );
        p.box_price = Some(72.0);
        assert_eq!(
            menu_box(&p),
            Some(MenuBox {
                quantity: 12,
                price: 72.0
            })
        );
        p.box_cost_price = Some(80.0);
        assert_eq!(menu_box(&p), None);
        p.has_box_price = false;
        assert_eq!(menu_box(&p), None);
    }
}
