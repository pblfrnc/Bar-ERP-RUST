//! Backup do banco do BarERP. Tradução de `server/src/services/backupService.ts`, mais a restauração.
//!
//! - Snapshot consistente com `VACUUM INTO` (seguro com o sistema gravando) em
//!   `<pasta do banco>/backups/backup_AAAA-MM-DD_HHmm.db`.
//! - Cópia para uma pasta externa (Google Drive / OneDrive / pen drive) como `bar-erp-backup_*.db`.
//! - Retenção: tudo dos 2 dias mais recentes + o último de cada dia anterior, até `keep` dias.
//! - Configuração em `<pasta do banco>/backup-settings.json`, no mesmo formato do servidor Node.
//! - [`restore`]: troca o banco por um backup validado, guardando antes uma cópia do atual.

mod files;
mod restore;
mod service;
mod settings;

pub use files::{plan_prune, BackupFileInfo};
pub use restore::{restore, validate_backup_file, RestoreResult};
pub use service::{BackupEntry, BackupResult, BackupService, BackupSource, BackupStatus};
pub use settings::{BackupSettings, BackupSettingsInput};

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// Mensagem pronta para mostrar ao operador.
    #[error("{0}")]
    Message(String),
    #[error("Erro no banco de dados: {0}")]
    Db(#[from] sqlx::Error),
    #[error("Erro de arquivo: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) fn msg(s: impl Into<String>) -> BackupError {
    BackupError::Message(s.into())
}
