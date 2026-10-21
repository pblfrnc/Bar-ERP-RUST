//! Publicação contra um site falso (wiremock): nenhuma chamada real ao Netlify.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use barerp_menu_publish::*;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[derive(Default)]
struct Data {
    settings: DigitalMenuSettings,
    source: MenuSource,
    images: HashMap<String, (ImageRef, ImageData)>,
    reset_images: usize,
}

/// Banco em memória.
#[derive(Clone, Default)]
struct MemStore(Arc<Mutex<Data>>);

impl MemStore {
    fn with(f: impl FnOnce(&mut Data)) -> Self {
        let s = MemStore::default();
        f(&mut s.0.lock().unwrap());
        s
    }
    fn get<T>(&self, f: impl FnOnce(&Data) -> T) -> T {
        f(&self.0.lock().unwrap())
    }
}

impl MenuStore for MemStore {
    async fn settings(&self) -> Result<DigitalMenuSettings, StoreError> {
        Ok(self.get(|d| d.settings.clone()))
    }
    async fn default_title(&self) -> Result<String, StoreError> {
        Ok("Bar do Zé".into())
    }
    async fn menu_source(&self) -> Result<MenuSource, StoreError> {
        Ok(self.get(|d| d.source.clone()))
    }
    async fn set_credentials(&self, store_id: String, secret: String) -> Result<(), StoreError> {
        let mut d = self.0.lock().unwrap();
        d.settings.store_id = Some(store_id);
        d.settings.publish_secret = Some(secret);
        d.settings.last_hash = None;
        Ok(())
    }
    async fn reset_published_images(&self) -> Result<(), StoreError> {
        let mut d = self.0.lock().unwrap();
        d.reset_images += 1;
        d.images
            .values_mut()
            .for_each(|(r, _)| r.published_hash = None);
        Ok(())
    }
    async fn images(&self, ids: Vec<String>) -> Result<Vec<ImageRef>, StoreError> {
        Ok(self.get(|d| {
            ids.iter()
                .filter_map(|id| d.images.get(id).map(|(r, _)| r.clone()))
                .collect()
        }))
    }
    async fn image_data(&self, id: String) -> Result<Option<ImageData>, StoreError> {
        Ok(self.get(|d| d.images.get(&id).map(|(_, data)| data.clone())))
    }
    async fn mark_image_published(&self, id: String, hash: String) -> Result<(), StoreError> {
        let mut d = self.0.lock().unwrap();
        if let Some((r, _)) = d.images.get_mut(&id) {
            r.published_hash = Some(hash);
        }
        Ok(())
    }
    async fn record_published(
        &self,
        last_hash: Option<String>,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut d = self.0.lock().unwrap();
        d.settings.last_hash = last_hash;
        d.settings.last_published_at = Some(at);
        d.settings.last_error = None;
        Ok(())
    }
    async fn record_error(&self, message: String) -> Result<(), StoreError> {
        self.0.lock().unwrap().settings.last_error = Some(message);
        Ok(())
    }
    async fn update_settings(&self, u: SettingsUpdate) -> Result<(), StoreError> {
        let mut d = self.0.lock().unwrap();
        let s = &mut d.settings;
        s.site_url = u.site_url;
        s.title = u.title;
        s.message = u.message;
        s.hidden_category_ids = u.hidden_category_ids;
        s.enabled = u.enabled;
        s.hide_sold_out = u.hide_sold_out;
        s.ignore_stock = u.ignore_stock;
        s.last_error = None;
        if u.clear_last_hash {
            s.last_hash = None;
        }
        Ok(())
    }
}

fn product(id: &str, name: &str, price: f64) -> SourceProduct {
    SourceProduct {
        id: id.into(),
        category_id: "cat".into(),
        name: name.into(),
        price,
        is_active: true,
        show_in_menu: true,
        kds_station: "BAR".into(),
        track_stock: false,
        ..Default::default()
    }
}

fn image(id: &str, bytes: &[u8], published: Option<&str>) -> (String, (ImageRef, ImageData)) {
    let hash = image_hash(bytes);
    (
        id.to_string(),
        (
            ImageRef {
                product_id: id.into(),
                hash,
                published_hash: published.map(str::to_string),
            },
            ImageData {
                data: bytes.to_vec(),
                mime_type: "image/jpeg".into(),
            },
        ),
    )
}

/// Loja ligada, apontando para o site falso, com 2 produtos e foto em um deles.
fn store(site: &str) -> MemStore {
    MemStore::with(|d| {
        d.settings = DigitalMenuSettings {
            enabled: true,
            site_url: Some(site.into()),
            hidden_category_ids: "[]".into(),
            ..Default::default()
        };
        d.source.categories = vec![SourceCategory {
            id: "cat".into(),
            name: "Bebidas".into(),
            ..Default::default()
        }];
        d.source.products = vec![product("p1", "Cerveja", 12.0), product("p2", "Água", 4.5)];
        d.images = HashMap::from([image("p1", b"foto-cerveja", None)]);
        d.source.image_hashes = d
            .images
            .iter()
            .map(|(k, (r, _))| (k.clone(), r.hash.clone()))
            .collect();
    })
}

fn ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "success": true }))
}

async fn requests(server: &MockServer, m: &str, path_prefix: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == m && r.url.path().starts_with(path_prefix))
        .collect()
}

#[tokio::test]
async fn primeira_publicacao_cria_loja_sobe_fotos_e_cardapio() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path_regex(r"^/api/menu/[a-f0-9]{20}(/img/[a-f0-9]{32})?$"))
        .respond_with(ok())
        .mount(&server)
        .await;
    let st = store(&server.uri());
    let publisher = MenuPublisher::new(st.clone());

    publisher.publish(false).await.unwrap();

    let (store_id, secret) = st.get(|d| {
        (
            d.settings.store_id.clone().unwrap(),
            d.settings.publish_secret.clone().unwrap(),
        )
    });
    assert_eq!(store_id.len(), 20);
    assert_eq!(secret.len(), 64);
    assert_eq!(
        st.get(|d| d.reset_images),
        1,
        "loja nova: fotos sobem de novo"
    );

    let imgs = requests(&server, "PUT", &format!("/api/menu/{store_id}/img/")).await;
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].body, b"foto-cerveja");
    assert_eq!(imgs[0].headers.get("content-type").unwrap(), "image/jpeg");
    assert_eq!(
        imgs[0]
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {secret}")
    );

    let menus = requests(&server, "PUT", &format!("/api/menu/{store_id}")).await;
    let menu_put: Vec<_> = menus
        .iter()
        .filter(|r| !r.url.path().contains("/img/"))
        .collect();
    assert_eq!(menu_put.len(), 1);
    let body: Value = serde_json::from_slice(&menu_put[0].body).unwrap();
    // Ordem das chaves do `{ ...menu, products, updatedAt }` do Node (Value ordenaria; confere no texto)
    let raw = String::from_utf8(menu_put[0].body.clone()).unwrap();
    let pos: Vec<usize> = [
        "\"version\"",
        "\"store\"",
        "\"categories\"",
        "\"products\"",
        "\"promotions\"",
        "\"updatedAt\"",
    ]
    .iter()
    .map(|k| raw.find(k).unwrap())
    .collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{raw}");
    assert_eq!(body["store"]["name"], "Bar do Zé");
    // Ordem binária do SQLite: "Cerveja" antes de "Água"
    assert_eq!(body["products"][0]["name"], "Cerveja");
    assert_eq!(
        body["products"][0]["image"],
        format!("/api/menu/{store_id}/img/{}", image_hash(b"foto-cerveja"))
    );
    assert_eq!(body["products"][0]["price"], json!(12));

    let s = st.get(|d| d.settings.clone());
    assert!(s.last_hash.is_some() && s.last_published_at.is_some() && s.last_error.is_none());

    // Nada mudou: não envia de novo (economiza a cota do Netlify)
    let before = server.received_requests().await.unwrap().len();
    publisher.publish(false).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    // Forçado envia, e a foto já publicada não sobe de novo
    publisher.publish(true).await.unwrap();
    let after = server.received_requests().await.unwrap();
    assert_eq!(after.len(), before + 1);
    assert!(!after.last().unwrap().url.path().contains("/img/"));

    // Status
    let status = publisher.status().await.unwrap();
    assert_eq!(
        status.menu_url.unwrap(),
        format!("{}/m/{store_id}", server.uri())
    );
    assert_eq!(status.default_title, "Bar do Zé");
    assert!(!status.publishing);
}

#[tokio::test]
async fn foto_que_falha_sai_sem_foto_e_tenta_de_novo() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path_regex(r"/img/"))
        .respond_with(
            ResponseTemplate::new(415)
                .set_body_json(json!({ "error": "Use foto JPEG, PNG ou WebP." })),
        )
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"^/api/menu/[a-f0-9]{20}$"))
        .respond_with(ok())
        .mount(&server)
        .await;
    let st = store(&server.uri());
    MenuPublisher::new(st.clone()).publish(false).await.unwrap();

    let put = requests(&server, "PUT", "/api/menu/")
        .await
        .into_iter()
        .find(|r| !r.url.path().contains("/img/"))
        .unwrap();
    let body: Value = serde_json::from_slice(&put.body).unwrap();
    assert!(body["products"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["image"].is_null()));
    // Sem hash gravado: a próxima conferência tenta a foto de novo
    assert_eq!(st.get(|d| d.settings.last_hash.clone()), None);
    assert_eq!(st.get(|d| d.images["p1"].0.published_hash.clone()), None);
}

#[tokio::test]
async fn foto_trocada_apaga_a_antiga() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ok())
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ok())
        .mount(&server)
        .await;
    let st = store(&server.uri());
    st.0.lock().unwrap().settings.store_id = Some("a1b2c3d4e5f60718293a".into());
    st.0.lock().unwrap().settings.publish_secret = Some("s".repeat(64));
    let old = "0123456789abcdef0123456789abcdef";
    st.0.lock()
        .unwrap()
        .images
        .get_mut("p1")
        .unwrap()
        .0
        .published_hash = Some(old.into());

    MenuPublisher::new(st.clone()).publish(false).await.unwrap();
    for _ in 0..100 {
        if !requests(&server, "DELETE", "/").await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let del = requests(&server, "DELETE", "/").await;
    assert_eq!(del.len(), 1);
    assert_eq!(
        del[0].url.path(),
        format!("/api/menu/a1b2c3d4e5f60718293a/img/{old}")
    );
    assert_eq!(
        st.get(|d| d.reset_images),
        0,
        "loja existente não reenvia todas as fotos"
    );
}

#[tokio::test]
async fn erro_do_site_vira_mensagem_e_fica_no_status() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path_regex(r"/img/"))
        .respond_with(ok())
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"^/api/menu/[a-f0-9]{20}$"))
        .respond_with(
            ResponseTemplate::new(413)
                .set_body_json(json!({ "error": "Cardápio grande demais para publicar." })),
        )
        .mount(&server)
        .await;
    let st = store(&server.uri());
    let p = MenuPublisher::new(st.clone());
    let e = p.publish(false).await.unwrap_err();
    assert_eq!(e.to_string(), "Cardápio grande demais para publicar.");
    assert_eq!(
        p.status().await.unwrap().last_error.unwrap(),
        "Cardápio grande demais para publicar."
    );

    // Sem corpo de erro: mensagem com o status
    server.reset().await;
    Mock::given(method("PUT"))
        .and(path_regex(r"/img/"))
        .respond_with(ok())
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let e = p.publish(true).await.unwrap_err();
    assert_eq!(e.to_string(), "O site do cardápio respondeu HTTP 502.");
}

#[tokio::test]
async fn sem_conexao_mensagem_amigavel() {
    // Porta fechada no próprio computador
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let st = store(&format!("http://127.0.0.1:{port}"));
    st.0.lock().unwrap().images.clear();
    let e = MenuPublisher::new(st).publish(false).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "Não foi possível conectar ao site do cardápio. Será tentado de novo em alguns minutos."
    );
}

#[tokio::test]
async fn desligado_nao_faz_nada() {
    let server = MockServer::start().await;
    let st = store(&server.uri());
    st.0.lock().unwrap().settings.enabled = false;
    MenuPublisher::new(st.clone()).publish(true).await.unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(st.get(|d| d.settings.store_id.clone()), None);
}

#[tokio::test]
async fn publicacoes_simultaneas_e_espera_depois_do_cadastro() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ok().set_delay(Duration::from_millis(100)))
        .mount(&server)
        .await;
    let st = store(&server.uri());
    st.0.lock().unwrap().images.clear();
    let p = MenuPublisher::with_timings(
        st.clone(),
        Duration::from_millis(80),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );

    // Durante um envio, o segundo pedido espera e vira uma rodada no final (que não reenvia: nada mudou)
    let (a, b) = tokio::join!(p.publish(true), p.publish(false));
    a.unwrap();
    b.unwrap();
    assert_eq!(requests(&server, "PUT", "/").await.len(), 1);

    // Três mudanças seguidas no cadastro = uma publicação só, depois da espera
    st.0.lock().unwrap().source.products[0].price = 13.0;
    p.schedule_publish();
    p.schedule_publish();
    p.schedule_publish();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        requests(&server, "PUT", "/").await.len(),
        1,
        "ainda esperando"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(requests(&server, "PUT", "/").await.len(), 2);
}

#[tokio::test]
async fn salvar_configuracoes_troca_de_site_e_desliga() {
    let old_site = MockServer::start().await;
    let new_site = MockServer::start().await;
    for s in [&old_site, &new_site] {
        Mock::given(method("PUT")).respond_with(ok()).mount(s).await;
        Mock::given(method("DELETE"))
            .respond_with(ok())
            .mount(s)
            .await;
    }
    let st = store(&old_site.uri());
    let p = MenuPublisher::new(st.clone());
    p.publish(false).await.unwrap();
    let store_id = st.get(|d| d.settings.store_id.clone().unwrap());

    // Endereço inválido
    let e = p
        .save_settings(&json!({ "siteUrl": "http://meubar.com" }))
        .await
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "Endereço do site inválido. Use https://nome-do-site.netlify.app"
    );

    // Troca de site: tira do ar no antigo, fotos voltam a subir, publica no novo
    let long_title = "T".repeat(100);
    let r = p
        .save_settings(&json!({
            "siteUrl": format!("{}//", new_site.uri()),
            "title": format!("  {long_title}  "),
            "hiddenCategoryIds": ["x", 5],
            "hideSoldOut": true,
        }))
        .await
        .unwrap();
    assert_eq!(r.publish_error, None);
    let del = requests(&old_site, "DELETE", "/").await;
    assert_eq!(del.len(), 1);
    assert_eq!(del[0].url.path(), format!("/api/menu/{store_id}"));
    assert_eq!(
        requests(&new_site, "PUT", &format!("/api/menu/{store_id}/img/"))
            .await
            .len(),
        1
    );
    let s = st.get(|d| d.settings.clone());
    assert_eq!(s.site_url.as_deref(), Some(new_site.uri().as_str()));
    assert_eq!(s.title.as_deref().unwrap().len(), 80);
    assert_eq!(s.hidden_category_ids, r#"["x","5"]"#);
    assert!(s.hide_sold_out && s.enabled);
    assert_eq!(r.status.site_url, new_site.uri());
    assert_eq!(r.status.hidden_category_ids, vec!["x", "5"]);

    // Desligar: tira do ar e não publica
    let puts_before = requests(&new_site, "PUT", "/").await.len();
    let r = p.save_settings(&json!({ "enabled": false })).await.unwrap();
    assert!(!r.status.enabled);
    assert_eq!(
        requests(&new_site, "DELETE", &format!("/api/menu/{store_id}"))
            .await
            .len(),
        1
    );
    assert_eq!(requests(&new_site, "PUT", "/").await.len(), puts_before);
    assert_eq!(st.get(|d| d.settings.last_hash.clone()), None);
}

#[tokio::test]
async fn erro_ao_publicar_depois_de_salvar_vem_no_resultado() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(header("content-type", "application/json"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({ "error": "Cardápio inválido." })),
        )
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("/img/"))
        .respond_with(ok())
        .mount(&server)
        .await;
    let st = store(&server.uri());
    st.0.lock().unwrap().settings.enabled = false;
    let r = MenuPublisher::new(st)
        .save_settings(&json!({ "enabled": true, "message": "  Wi-Fi: bar123  " }))
        .await
        .unwrap();
    assert_eq!(r.publish_error.as_deref(), Some("Cardápio inválido."));
    assert_eq!(r.status.last_error.as_deref(), Some("Cardápio inválido."));
    assert_eq!(r.status.message, "Wi-Fi: bar123");
}
