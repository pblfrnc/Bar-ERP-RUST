//! O cardápio montado em Rust tem que ser byte a byte o do Node, senão o hash muda e o PC republica à toa.
//! `fixtures/node_menu_expected.json` foi gerado rodando o `buildMenu` original de
//! `server/src/services/digitalMenu.ts` (com um Prisma falso lendo `fixtures/menu_source.json`, TZ=America/Sao_Paulo).

use std::collections::HashMap;

use barerp_menu_publish::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    settings: FxSettings,
    fiscal: FxFiscal,
    system: FxSystem,
    categories: Vec<FxCategory>,
    products: Vec<FxProduct>,
    promotions: Vec<FxPromotion>,
    images: Vec<FxImage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxSettings {
    store_id: String,
    title: Option<String>,
    message: Option<String>,
    hidden_category_ids: String,
    hide_sold_out: bool,
    ignore_stock: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxFiscal {
    nome_fantasia: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxSystem {
    restaurant_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxCategory {
    id: String,
    name: String,
    icon: Option<String>,
    sort_order: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxOption {
    name: String,
    price: f64,
    is_active: bool,
    sort_order: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxGroup {
    name: String,
    is_active: bool,
    min_select: i64,
    max_select: i64,
    options: Vec<FxOption>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxLink {
    sort_order: i64,
    group: FxGroup,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxProduct {
    id: String,
    category_id: String,
    name: String,
    description: Option<String>,
    price: f64,
    is_active: bool,
    show_in_menu: bool,
    kds_station: String,
    track_stock: bool,
    stock: i64,
    cost_price: Option<f64>,
    has_box_price: bool,
    box_price: Option<f64>,
    box_quantity: Option<i64>,
    box_cost_price: Option<f64>,
    modifier_groups: Vec<FxLink>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxPromotion {
    id: String,
    name: String,
    is_active: bool,
    days_of_week: String,
    start_time: String,
    end_time: String,
    discount_type: String,
    value: f64,
    product_ids: String,
    category_ids: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FxImage {
    product_id: String,
    hash: String,
}

#[derive(Deserialize)]
struct Expected {
    json: String,
    hash: String,
}

fn load() -> (DigitalMenuSettings, MenuSource) {
    let fx: Fixture = serde_json::from_str(include_str!("fixtures/menu_source.json")).unwrap();
    let settings = DigitalMenuSettings {
        enabled: true,
        store_id: Some(fx.settings.store_id),
        publish_secret: Some("segredo".into()),
        title: fx.settings.title,
        message: fx.settings.message,
        hidden_category_ids: fx.settings.hidden_category_ids,
        hide_sold_out: fx.settings.hide_sold_out,
        ignore_stock: fx.settings.ignore_stock,
        ..Default::default()
    };
    let source = MenuSource {
        categories: fx
            .categories
            .into_iter()
            .map(|c| SourceCategory {
                id: c.id,
                name: c.name,
                icon: c.icon,
                sort_order: c.sort_order,
            })
            .collect(),
        products: fx
            .products
            .into_iter()
            .map(|p| SourceProduct {
                id: p.id,
                category_id: p.category_id,
                name: p.name,
                description: p.description,
                price: p.price,
                is_active: p.is_active,
                show_in_menu: p.show_in_menu,
                kds_station: p.kds_station,
                track_stock: p.track_stock,
                stock: p.stock,
                cost_price: p.cost_price,
                has_box_price: p.has_box_price,
                box_price: p.box_price,
                box_quantity: p.box_quantity,
                box_cost_price: p.box_cost_price,
                modifier_groups: p
                    .modifier_groups
                    .into_iter()
                    .map(|l| SourceGroupLink {
                        sort_order: l.sort_order,
                        group: SourceModifierGroup {
                            name: l.group.name,
                            is_active: l.group.is_active,
                            min_select: l.group.min_select,
                            max_select: l.group.max_select,
                            options: l
                                .group
                                .options
                                .into_iter()
                                .map(|o| SourceOption {
                                    name: o.name,
                                    price: o.price,
                                    is_active: o.is_active,
                                    sort_order: o.sort_order,
                                })
                                .collect(),
                        },
                    })
                    .collect(),
            })
            .collect(),
        promotions: fx
            .promotions
            .into_iter()
            .map(|p| SourcePromotion {
                id: p.id,
                name: p.name,
                is_active: p.is_active,
                days_of_week: p.days_of_week,
                start_time: p.start_time,
                end_time: p.end_time,
                discount_type: p.discount_type,
                value: p.value,
                product_ids: p.product_ids,
                category_ids: p.category_ids,
            })
            .collect(),
        image_hashes: fx
            .images
            .into_iter()
            .map(|i| (i.product_id, i.hash))
            .collect::<HashMap<_, _>>(),
        default_title: default_title(
            fx.fiscal.nome_fantasia.as_deref(),
            fx.system.restaurant_name.as_deref(),
        ),
        timezone_offset: 180,
    };
    (settings, source)
}

#[test]
fn json_e_hash_iguais_ao_node() {
    let expected: Expected =
        serde_json::from_str(include_str!("fixtures/node_menu_expected.json")).unwrap();
    let (settings, source) = load();
    let menu = build_menu(&settings, &source);
    let json = serde_json::to_string(&menu).unwrap();
    assert_eq!(json, expected.json);
    assert_eq!(menu_hash(&menu), expected.hash);
}

#[test]
fn ordem_da_entrada_nao_importa() {
    let (settings, mut source) = load();
    let a = menu_hash(&build_menu(&settings, &source));
    source.products.reverse();
    source.categories.reverse();
    source.promotions.reverse();
    for p in &mut source.products {
        p.modifier_groups.reverse();
        for l in &mut p.modifier_groups {
            l.group.options.reverse();
        }
    }
    assert_eq!(menu_hash(&build_menu(&settings, &source)), a);
}

#[test]
fn esconder_esgotados_e_ignorar_estoque() {
    let (mut settings, source) = load();
    let ids = |s: &DigitalMenuSettings| -> Vec<(String, bool)> {
        build_menu(s, &source)
            .products
            .into_iter()
            .map(|p| (p.id, p.sold_out))
            .collect()
    };
    let normal = ids(&settings);
    assert!(
        normal.contains(&("p-cerveja".into(), true)),
        "bar com estoque 0 = esgotado"
    );
    assert!(
        normal.contains(&("p-fritas".into(), false)),
        "cozinha nunca esgota"
    );
    assert!(
        normal.contains(&("p-refri".into(), false)),
        "sem controle de estoque"
    );

    settings.hide_sold_out = true;
    assert!(!ids(&settings).iter().any(|(id, _)| id == "p-cerveja"));

    settings.ignore_stock = true;
    assert!(ids(&settings).iter().all(|(_, sold)| !sold));
}
