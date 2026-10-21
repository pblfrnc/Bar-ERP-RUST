//! Licença do BarERP, tradução de `server/src/routes/settings.ts` (GET `/api/settings` e `/license/*`) e do
//! `LicenseModal.tsx`. O cliente continua com a mesma licença depois da troca porque:
//! - o ID da máquina é calculado como o `node-machine-id` (`machineIdSync()`: SHA-256 do GUID do Windows);
//! - a chave offline `EXP-AAAAMMDD-MAC8-SIG8` e a chave legada (e-mail + HMAC) usam o mesmo segredo;
//! - o painel de licenças no Render é consultado na mesma rota (`/api/v1/licenses/check/<id>`).
//!
//! Diferenças de propósito em relação ao Node:
//! - resposta do painel que não é JSON vira erro com mensagem (o Node respondia 500 com o texto da exceção);
//! - a consulta em segundo plano não roda junto com a leitura do status: quem chama decide (`should_check_remote`).

pub mod keys;
pub mod machine;
pub mod remote;
pub mod service;

pub use service::{activate_offline, background_check, status, sync_remote, DeveloperContact, LicenseStatus, SyncOutcome};

/// Endereço padrão do painel de licenças (Render), como no Node.
pub const DEFAULT_SERVER_URL: &str = "https://bar-erp-licensas.onrender.com";

/// Segredo das chaves (o mesmo do Node: `LICENSE_SECRET_KEY` ou o padrão).
pub fn secret_key() -> String {
    std::env::var("LICENSE_SECRET_KEY").unwrap_or_else(|_| "BAR_ERP_SUPER_SECRET_KEY_2026".into())
}

#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("{0}")]
    Db(#[from] barerp_db::DbError),
    #[error("{0}")]
    Sql(#[from] sqlx::Error),
    /// Mensagem pronta para mostrar na tela
    #[error("{0}")]
    Message(String),
}

impl LicenseError {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

pub type LicenseResult<T> = Result<T, LicenseError>;
