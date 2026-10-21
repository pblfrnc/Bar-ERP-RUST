//! Uma página por área do menu do app original. Cada sessão/pessoa cuida dos próprios arquivos:
//! `src/pages/<area>.rs` + `ui/pages/<area>.slint` + `paridade/<area>.md`.

use crate::ctx::AppCtx;

pub mod caixa;
pub mod cardapio;
pub mod fiscal;
pub mod import_legacy;
pub mod kds;
pub mod mesas;
pub mod retaguarda;
pub mod update;

/// Liga os callbacks de todas as páginas (chamado uma vez ao abrir o app).
pub fn bind_all(ctx: &AppCtx) {
    mesas::bind(ctx);
    caixa::bind(ctx);
    kds::bind(ctx);
    cardapio::bind(ctx);
    fiscal::bind(ctx);
    retaguarda::bind(ctx);
    import_legacy::bind(ctx);
    update::bind(ctx);
}
