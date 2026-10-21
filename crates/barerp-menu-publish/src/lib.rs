//! Cardápio digital por QR na mesa. Tradução de `server/src/services/digitalMenu.ts`.
//!
//! O PC do bar monta o cardápio (só dados públicos: nome, descrição, preço, adicionais e promoções)
//! e publica no site do Netlify (pasta `menu/`, funções `menu.mts` e `menu-image.mts`). O cliente
//! abre `<site>/m/<storeId>?mesa=N`. Publica sozinho depois de alterações no cadastro e confere a
//! cada 5 minutos (estoque/esgotados); só envia quando algo mudou, para não gastar a cota do Netlify.
//!
//! - [`build_menu`] / [`menu_hash`]: o cardápio publicado, com o mesmo JSON (e o mesmo hash) do Node.
//! - [`MenuPublisher`]: publicação (uma por vez), fotos, agendador, status e configurações.
//! - [`MenuStore`]: o que o publicador precisa do banco; quem integra implementa (sem depender do barerp-db).

mod client;
pub mod db_store;
mod menu;
mod model;
mod publisher;
mod settings;

pub use menu::{build_menu, default_title, image_hash, js_timezone_offset, menu_hash};
pub use model::{
    MenuBox, MenuCategory, MenuGroup, MenuKind, MenuOption, MenuProduct, MenuPromotion, MenuSource,
    MenuStoreInfo, PublishedMenu, SourceCategory, SourceGroupLink, SourceModifierGroup,
    SourceOption, SourceProduct, SourcePromotion,
};
pub use publisher::{
    ImageData, ImageRef, MenuPublisher, MenuStore, SaveSettingsResult, SettingsUpdate, StoreError,
};
pub use settings::{
    effective_site_url, DigitalMenuSettings, DigitalMenuStatus, DEFAULT_MENU_SITE_URL,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MenuError {
    /// Mensagem pronta para o operador (já traduzida, como o `friendlyError` do Node).
    #[error("{0}")]
    Message(String),
    /// Falha ao ler ou gravar no banco (implementação do [`MenuStore`]).
    #[error("Erro no banco de dados: {0}")]
    Store(String),
}
