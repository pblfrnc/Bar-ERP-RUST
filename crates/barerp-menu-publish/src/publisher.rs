//! Publicação do cardápio: uma por vez, fotos antes do cardápio, agendador e configurações.

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use rand::RngCore;
use reqwest::Method;
use serde::Serialize;
use tokio::sync::watch;

use crate::client::{Site, SiteClient};
use crate::menu::{build_menu, js_timezone_offset, menu_hash};
use crate::model::{MenuProduct, MenuSource, PublishBody};
use crate::settings::{
    effective_site_url, js_string, parse_ids, valid_site_url, DigitalMenuSettings,
    DigitalMenuStatus, DEFAULT_MENU_SITE_URL,
};
use crate::MenuError;

pub type StoreError = Box<dyn std::error::Error + Send + Sync>;

/// Foto de um produto e o hash que o site já tem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub product_id: String,
    pub hash: String,
    pub published_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    pub data: Vec<u8>,
    pub mime_type: String,
}

/// Campos que a tela de configurações grava (e sempre `lastError = NULL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsUpdate {
    pub site_url: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    /// JSON (`["id1","id2"]`)
    pub hidden_category_ids: String,
    pub enabled: bool,
    pub hide_sold_out: bool,
    pub ignore_stock: bool,
    /// Também zera `lastHash` (trocou de site ou desligou)
    pub clear_last_hash: bool,
}

/// O que o publicador precisa do banco. Quem integra implementa com as consultas equivalentes às do
/// Node (`digitalMenu.ts`); cada método é uma operação curta e independente.
pub trait MenuStore: Send + Sync + 'static {
    /// Linha `DigitalMenuSettings` `id = 'default'` (cria com os padrões se não existir).
    fn settings(&self) -> impl Future<Output = Result<DigitalMenuSettings, StoreError>> + Send;
    /// Nome do bar para o título: veja [`crate::default_title`].
    fn default_title(&self) -> impl Future<Output = Result<String, StoreError>> + Send;
    /// Categorias, produtos (com grupos e opções), promoções e hashes das fotos. Pode vir sem filtro
    /// nem ordem; `default_title` e `timezone_offset` são preenchidos pelo publicador.
    fn menu_source(&self) -> impl Future<Output = Result<MenuSource, StoreError>> + Send;
    /// Grava `storeId` e `publishSecret` novos e zera `lastHash`.
    fn set_credentials(
        &self,
        store_id: String,
        secret: String,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// `ProductImage.publishedHash = NULL` em todas (site novo não tem as fotos).
    fn reset_published_images(&self) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Fotos destes produtos.
    fn images(
        &self,
        product_ids: Vec<String>,
    ) -> impl Future<Output = Result<Vec<ImageRef>, StoreError>> + Send;
    fn image_data(
        &self,
        product_id: String,
    ) -> impl Future<Output = Result<Option<ImageData>, StoreError>> + Send;
    fn mark_image_published(
        &self,
        product_id: String,
        hash: String,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    /// Publicou: `lastHash`, `lastPublishedAt = agora`, `lastError = NULL`.
    fn record_published(
        &self,
        last_hash: Option<String>,
        at: chrono::DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn record_error(&self, message: String) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn update_settings(
        &self,
        update: SettingsUpdate,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// Resposta de salvar as configurações: o status novo e o erro da publicação imediata (se houve).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsResult {
    pub status: DigitalMenuStatus,
    pub publish_error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Timings {
    debounce: Duration,
    first_check: Duration,
    every: Duration,
}

const DEFAULT_TIMINGS: Timings = Timings {
    debounce: Duration::from_secs(8),
    first_check: Duration::from_secs(20),
    every: Duration::from_secs(5 * 60),
};

type Outcome = Option<Result<(), MenuError>>;

#[derive(Default)]
struct State {
    running: Option<watch::Receiver<Outcome>>,
    run_again: bool,
}

struct Inner<S> {
    store: S,
    client: SiteClient,
    state: Mutex<State>,
    debounce_gen: AtomicU64,
    scheduler_started: AtomicBool,
    timings: Timings,
}

/// Barato de clonar (tudo atrás de um `Arc`).
pub struct MenuPublisher<S> {
    inner: Arc<Inner<S>>,
}

impl<S> Clone for MenuPublisher<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

fn store_err(e: StoreError) -> MenuError {
    MenuError::Store(e.to_string())
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

impl<S: MenuStore> MenuPublisher<S> {
    pub fn new(store: S) -> Self {
        Self::build(store, DEFAULT_TIMINGS)
    }

    /// Tempos curtos para testes (espera após cadastro, primeira conferência, intervalo).
    #[doc(hidden)]
    pub fn with_timings(
        store: S,
        debounce: Duration,
        first_check: Duration,
        every: Duration,
    ) -> Self {
        Self::build(
            store,
            Timings {
                debounce,
                first_check,
                every,
            },
        )
    }

    fn build(store: S, timings: Timings) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                client: SiteClient::new(),
                state: Mutex::new(State::default()),
                debounce_gen: AtomicU64::new(0),
                scheduler_started: AtomicBool::new(false),
                timings,
            }),
        }
    }

    pub fn store(&self) -> &S {
        &self.inner.store
    }

    /// Publica se o cardápio mudou (`force` publica de qualquer jeito). Uma publicação por vez:
    /// pedidos durante o envio esperam o mesmo resultado e viram uma nova rodada no final.
    pub async fn publish(&self, force: bool) -> Result<(), MenuError> {
        let mut rx = {
            let mut state = self.inner.state.lock().unwrap();
            match state.running.clone() {
                Some(rx) => {
                    state.run_again = true;
                    rx
                }
                None => {
                    let (tx, rx) = watch::channel(None);
                    state.running = Some(rx.clone());
                    let this = self.clone();
                    tokio::spawn(async move {
                        let mut force = force;
                        let result = loop {
                            let r = this.do_publish(force).await;
                            let mut state = this.inner.state.lock().unwrap();
                            if r.is_ok() && state.run_again {
                                state.run_again = false;
                                force = false;
                                continue;
                            }
                            state.running = None;
                            state.run_again = false;
                            break r;
                        };
                        let _ = tx.send(Some(result));
                    });
                    rx
                }
            }
        };
        let done = rx.wait_for(Option::is_some).await.map(|r| r.clone());
        match done {
            Ok(r) => r.expect("esperou por Some"),
            Err(_) => Err(MenuError::Message(
                "A publicação do cardápio foi interrompida.".into(),
            )),
        }
    }

    async fn do_publish(&self, force: bool) -> Result<(), MenuError> {
        let store = &self.inner.store;
        let mut row = store.settings().await.map_err(store_err)?;
        if !row.enabled {
            return Ok(());
        }
        if non_empty(&row.store_id).is_none() || non_empty(&row.publish_secret).is_none() {
            let (id, secret) = (random_hex(10), random_hex(32));
            store
                .set_credentials(id.clone(), secret.clone())
                .await
                .map_err(store_err)?;
            // Loja nova: as fotos precisam subir de novo
            store.reset_published_images().await.map_err(store_err)?;
            row.store_id = Some(id);
            row.publish_secret = Some(secret);
            row.last_hash = None;
        }

        let mut source = store.menu_source().await.map_err(store_err)?;
        source.default_title = store.default_title().await.map_err(store_err)?;
        source.timezone_offset = js_timezone_offset();
        let menu = build_menu(&row, &source);
        let hash = menu_hash(&menu);
        if !force && row.last_hash.as_deref() == Some(hash.as_str()) {
            return Ok(());
        }

        let site = Site {
            url: effective_site_url(&row),
            store_id: row.store_id.as_deref().unwrap_or_default(),
            secret: row.publish_secret.as_deref().unwrap_or_default(),
        };
        let sent = async {
            // Fotos antes do cardápio; a que falhar sai sem foto agora e o hash não é gravado, para tentar de novo
            let with_image: Vec<String> = menu
                .products
                .iter()
                .filter(|p| p.image.is_some())
                .map(|p| p.id.clone())
                .collect();
            let failed = self.sync_images(&site, with_image).await?;
            let products: Vec<MenuProduct> = menu
                .products
                .iter()
                .map(|p| {
                    if failed.contains(&p.id) {
                        MenuProduct {
                            image: None,
                            ..p.clone()
                        }
                    } else {
                        p.clone()
                    }
                })
                .collect();
            let body = PublishBody {
                version: menu.version,
                store: &menu.store,
                categories: &menu.categories,
                products: &products,
                promotions: &menu.promotions,
                updated_at: Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            };
            let json = serde_json::to_string(&body).expect("cardápio sempre serializa");
            self.inner
                .client
                .menu(&site, Method::PUT, Some(json))
                .await?;
            let last_hash = failed.is_empty().then(|| hash.clone());
            store
                .record_published(last_hash, Utc::now())
                .await
                .map_err(store_err)
        }
        .await;
        if let Err(e) = &sent {
            let _ = store.record_error(e.to_string()).await;
        }
        sent
    }

    /// Envia ao site as fotos que mudaram desde a última publicação. Devolve os produtos cuja foto não subiu.
    async fn sync_images(
        &self,
        site: &Site<'_>,
        product_ids: Vec<String>,
    ) -> Result<HashSet<String>, MenuError> {
        let store = &self.inner.store;
        let mut failed = HashSet::new();
        let pending = store.images(product_ids).await.map_err(store_err)?;
        for img in pending
            .into_iter()
            .filter(|i| i.published_hash.as_deref() != Some(i.hash.as_str()))
        {
            let sent = async {
                let Some(full) = store
                    .image_data(img.product_id.clone())
                    .await
                    .map_err(store_err)?
                else {
                    return Ok(());
                };
                self.inner
                    .client
                    .put_image(site, &img.hash, &full.mime_type, full.data)
                    .await?;
                store
                    .mark_image_published(img.product_id.clone(), img.hash.clone())
                    .await
                    .map_err(store_err)?;
                // A foto antiga não é mais usada (best effort)
                if let Some(old) = &img.published_hash {
                    let url = format!("{}/api/menu/{}/img/{old}", site.url, site.store_id);
                    let client = self.inner.client.clone();
                    let secret = site.secret.to_string();
                    tokio::spawn(async move { client.delete_image(url, secret).await });
                }
                Ok::<(), MenuError>(())
            }
            .await;
            if let Err(e) = sent {
                tracing::warn!(
                    "[Cardápio digital] Foto do produto {} não foi enviada: {e}",
                    img.product_id
                );
                failed.insert(img.product_id);
            }
        }
        Ok(failed)
    }

    /// Chamar depois de mudanças no cadastro (produtos, categorias, adicionais, promoções):
    /// espera 8s sem novas mudanças e publica.
    pub fn schedule_publish(&self) {
        let generation = self.inner.debounce_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(this.inner.timings.debounce).await;
            if this.inner.debounce_gen.load(Ordering::SeqCst) == generation {
                if let Err(e) = this.publish(false).await {
                    tracing::warn!("[Cardápio digital] {e}");
                }
            }
        });
    }

    /// Confere 20s depois de iniciar e a cada 5 minutos (só envia se mudou). Só inicia uma vez.
    pub fn spawn_scheduler(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.inner.scheduler_started.swap(true, Ordering::SeqCst) {
            return None;
        }
        let this = self.clone();
        let Timings {
            first_check, every, ..
        } = self.inner.timings;
        let start = tokio::time::Instant::now();
        Some(tokio::spawn(async move {
            let first = async {
                tokio::time::sleep(first_check).await;
                this.tick().await;
            };
            let schedule = async {
                let mut ticker = tokio::time::interval_at(start + every, every);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    this.tick().await;
                }
            };
            tokio::join!(first, schedule);
        }))
    }

    async fn tick(&self) {
        if let Err(e) = self.publish(false).await {
            tracing::warn!("[Cardápio digital] {e}");
        }
    }

    pub async fn status(&self) -> Result<DigitalMenuStatus, MenuError> {
        let store = &self.inner.store;
        let row = store.settings().await.map_err(store_err)?;
        let site = effective_site_url(&row);
        Ok(DigitalMenuStatus {
            enabled: row.enabled,
            site_url: site.clone(),
            custom_site_url: row.site_url.clone().unwrap_or_default(),
            menu_url: non_empty(&row.store_id).map(|id| format!("{site}/m/{id}")),
            store_id: row.store_id.clone(),
            title: row.title.clone().unwrap_or_default(),
            default_title: store.default_title().await.map_err(store_err)?,
            message: row.message.clone().unwrap_or_default(),
            hidden_category_ids: parse_ids(&row.hidden_category_ids),
            hide_sold_out: row.hide_sold_out,
            ignore_stock: row.ignore_stock,
            last_published_at: row
                .last_published_at
                .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
            last_error: row.last_error.clone(),
            publishing: self.inner.state.lock().unwrap().running.is_some(),
        })
    }

    /// Salva o corpo do `POST` das configurações (mesmas regras do Node). Ligado, publica na hora;
    /// o erro dessa publicação volta em `publish_error` (e fica no status), não como `Err`.
    pub async fn save_settings(
        &self,
        body: &serde_json::Value,
    ) -> Result<SaveSettingsResult, MenuError> {
        let store = &self.inner.store;
        let before = store.settings().await.map_err(store_err)?;
        let field = |key: &str| body.get(key).filter(|v| !v.is_null()).map(js_string);
        let flag =
            |key: &str, current: bool| body.get(key).and_then(|v| v.as_bool()).unwrap_or(current);

        let site_url = field("siteUrl")
            .or(before.site_url.clone())
            .unwrap_or_default();
        let site_url = site_url.trim().trim_end_matches('/').to_string();
        if !site_url.is_empty() && !valid_site_url(&site_url) {
            return Err(MenuError::Message(
                "Endereço do site inválido. Use https://nome-do-site.netlify.app".into(),
            ));
        }
        let cut = |s: String, n: usize| s.trim().chars().take(n).collect::<String>();
        let title = cut(
            field("title").or(before.title.clone()).unwrap_or_default(),
            80,
        );
        let message = cut(
            field("message")
                .or(before.message.clone())
                .unwrap_or_default(),
            300,
        );
        let hidden_category_ids = match body.get("hiddenCategoryIds") {
            Some(serde_json::Value::Array(ids)) => {
                serde_json::to_string(&ids.iter().map(js_string).collect::<Vec<_>>())
                    .expect("lista de textos")
            }
            _ => before.hidden_category_ids.clone(),
        };
        let enabled = flag("enabled", before.enabled);
        let hide_sold_out = flag("hideSoldOut", before.hide_sold_out);
        let ignore_stock = flag("ignoreStock", before.ignore_stock);

        // Trocou de site: o cardápio antigo sai do ar antes (best effort) e o novo recebe tudo
        let new_site = if site_url.is_empty() {
            DEFAULT_MENU_SITE_URL
        } else {
            site_url.as_str()
        };
        let site_changed = new_site != effective_site_url(&before);
        if let (true, Some(id), Some(secret)) = (
            before.enabled,
            non_empty(&before.store_id),
            non_empty(&before.publish_secret),
        ) {
            if !enabled || site_changed {
                let site = Site {
                    url: effective_site_url(&before),
                    store_id: id,
                    secret,
                };
                if let Err(e) = self.inner.client.menu(&site, Method::DELETE, None).await {
                    tracing::warn!("[Cardápio digital] Falha ao tirar do ar: {e}");
                }
            }
        }
        // Site novo não tem as fotos
        if site_changed {
            store.reset_published_images().await.map_err(store_err)?;
        }

        let opt = |s: String| (!s.is_empty()).then_some(s);
        store
            .update_settings(SettingsUpdate {
                site_url: opt(site_url),
                title: opt(title),
                message: opt(message),
                hidden_category_ids,
                enabled,
                hide_sold_out,
                ignore_stock,
                clear_last_hash: site_changed || !enabled,
            })
            .await
            .map_err(store_err)?;

        let publish_error = if enabled {
            self.publish(true).await.err().map(|e| e.to_string())
        } else {
            None
        };
        Ok(SaveSettingsResult {
            status: self.status().await?,
            publish_error,
        })
    }
}
