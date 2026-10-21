//! [`MenuStore`] sobre o `bar.db` (`barerp_db::digital_menu`).
//!
//! ```ignore
//! let publisher = MenuPublisher::new(DbMenuStore::new(db.clone()));
//! publisher.spawn_scheduler();
//! ```

use std::collections::HashMap;

use barerp_db::digital_menu::{self as dm, DigitalMenuRow, DigitalMenuUpdate};
use barerp_db::{Db, DbResult, DbTime};
use chrono::{DateTime, Utc};

use crate::menu::{default_title, image_hash};
use crate::model::{
    MenuSource, SourceCategory, SourceGroupLink, SourceModifierGroup, SourceOption, SourceProduct,
    SourcePromotion,
};
use crate::publisher::{ImageData, ImageRef, MenuStore, SettingsUpdate, StoreError};
use crate::settings::DigitalMenuSettings;

#[derive(Clone)]
pub struct DbMenuStore {
    db: Db,
}

impl DbMenuStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

/// `PUT /products/:id/image`: confere a foto, calcula o hash da URL e grava. Devolve o hash
/// (`{ imageHash }` no Node). Depois chame `MenuPublisher::schedule_publish`.
pub async fn save_product_image(db: &Db, product_id: &str, data: Vec<u8>) -> DbResult<String> {
    let hash = image_hash(&data);
    dm::put_product_image(db, product_id, data, &hash).await?;
    Ok(hash)
}

fn to_settings(r: DigitalMenuRow) -> DigitalMenuSettings {
    DigitalMenuSettings {
        enabled: r.enabled,
        site_url: r.site_url,
        store_id: r.store_id,
        publish_secret: r.publish_secret,
        title: r.title,
        message: r.message,
        hidden_category_ids: r.hidden_category_ids,
        hide_sold_out: r.hide_sold_out,
        ignore_stock: r.ignore_stock,
        last_published_at: r.last_published_at.map(|t| t.0),
        last_hash: r.last_hash,
        last_error: r.last_error,
    }
}

fn boxed(e: barerp_db::DbError) -> StoreError {
    Box::new(e)
}

impl MenuStore for DbMenuStore {
    async fn settings(&self) -> Result<DigitalMenuSettings, StoreError> {
        Ok(to_settings(dm::settings(&self.db).await.map_err(boxed)?))
    }

    async fn default_title(&self) -> Result<String, StoreError> {
        let (fantasia, restaurant) = dm::title_sources(&self.db).await.map_err(boxed)?;
        Ok(default_title(fantasia.as_deref(), restaurant.as_deref()))
    }

    async fn menu_source(&self) -> Result<MenuSource, StoreError> {
        let data = dm::menu_data(&self.db).await.map_err(boxed)?;
        let mut options: HashMap<String, Vec<SourceOption>> = HashMap::new();
        for o in data.options {
            options.entry(o.group_id).or_default().push(SourceOption {
                name: o.name,
                price: o.price,
                is_active: o.is_active,
                sort_order: o.sort_order,
            });
        }
        let mut links: HashMap<String, Vec<SourceGroupLink>> = HashMap::new();
        for l in data.group_links {
            links
                .entry(l.product_id)
                .or_default()
                .push(SourceGroupLink {
                    sort_order: l.link_order,
                    group: SourceModifierGroup {
                        name: l.name,
                        is_active: l.is_active,
                        min_select: l.min_select,
                        max_select: l.max_select,
                        options: options.get(&l.group_id).cloned().unwrap_or_default(),
                    },
                });
        }
        Ok(MenuSource {
            categories: data
                .categories
                .into_iter()
                .map(|c| SourceCategory {
                    id: c.id,
                    name: c.name,
                    icon: c.icon,
                    sort_order: c.sort_order,
                })
                .collect(),
            products: data
                .products
                .into_iter()
                .map(|p| SourceProduct {
                    modifier_groups: links.remove(&p.id).unwrap_or_default(),
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
                })
                .collect(),
            promotions: data
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
            image_hashes: data.image_hashes.into_iter().collect(),
            // Preenchidos pelo publicador
            default_title: String::new(),
            timezone_offset: 0,
        })
    }

    async fn set_credentials(&self, store_id: String, secret: String) -> Result<(), StoreError> {
        dm::set_credentials(&self.db, &store_id, &secret)
            .await
            .map_err(boxed)
    }

    async fn reset_published_images(&self) -> Result<(), StoreError> {
        dm::reset_published_images(&self.db).await.map_err(boxed)
    }

    async fn images(&self, product_ids: Vec<String>) -> Result<Vec<ImageRef>, StoreError> {
        Ok(dm::images(&self.db, &product_ids)
            .await
            .map_err(boxed)?
            .into_iter()
            .map(|r| ImageRef {
                product_id: r.product_id,
                hash: r.hash,
                published_hash: r.published_hash,
            })
            .collect())
    }

    async fn image_data(&self, product_id: String) -> Result<Option<ImageData>, StoreError> {
        Ok(dm::product_image(&self.db, &product_id)
            .await
            .map_err(boxed)?
            .map(|i| ImageData {
                data: i.data,
                mime_type: i.mime_type,
            }))
    }

    async fn mark_image_published(
        &self,
        product_id: String,
        hash: String,
    ) -> Result<(), StoreError> {
        dm::mark_image_published(&self.db, &product_id, &hash)
            .await
            .map_err(boxed)
    }

    async fn record_published(
        &self,
        last_hash: Option<String>,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        dm::record_published(&self.db, last_hash.as_deref(), DbTime(at))
            .await
            .map_err(boxed)
    }

    async fn record_error(&self, message: String) -> Result<(), StoreError> {
        dm::record_error(&self.db, &message).await.map_err(boxed)
    }

    async fn update_settings(&self, u: SettingsUpdate) -> Result<(), StoreError> {
        dm::update_settings(
            &self.db,
            DigitalMenuUpdate {
                site_url: u.site_url,
                title: u.title,
                message: u.message,
                hidden_category_ids: u.hidden_category_ids,
                enabled: u.enabled,
                hide_sold_out: u.hide_sold_out,
                ignore_stock: u.ignore_stock,
                clear_last_hash: u.clear_last_hash,
            },
        )
        .await
        .map_err(boxed)
    }
}
