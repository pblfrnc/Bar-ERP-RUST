//! Restauração de um backup (não existia no servidor Node).
//!
//! **Quem chama precisa fechar o pool do banco antes** (`pool.close().await`) e abrir de novo
//! depois: com conexões abertas o SQLite continuaria usando o arquivo antigo (e no Windows a
//! troca do arquivo falha).

use std::path::{Path, PathBuf};

use chrono::Local;
use serde::Serialize;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row, SqliteConnection};

use crate::files::stamp;
use crate::service::{remove_if_exists, tmp_of, vacuum_into};
use crate::{msg, BackupError};

/// Tabelas que todo banco do BarERP tem; um SQLite sem elas não é backup do sistema.
const REQUIRED_TABLES: [&str; 3] = ["Order", "Product", "SystemSettings"];
const SAFETY_PREFIX: &str = "antes-da-restauracao_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    /// Cópia do banco como estava antes da troca (`backups/antes-da-restauracao_*.db`).
    /// Não entra na retenção automática.
    pub safety_copy: Option<PathBuf>,
    pub restored_from: PathBuf,
}

async fn open(path: &Path, read_only: bool) -> Result<SqliteConnection, BackupError> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(read_only)
        .create_if_missing(false)
        // Sem journal_mode: o sqlx só mexe no modo do arquivo se pedirmos (o banco do sistema fica em WAL)
        .disable_statement_logging();
    let opts = if read_only {
        opts.immutable(true)
    } else {
        opts
    };
    Ok(opts.connect().await?)
}

/// Confere se o arquivo é um banco SQLite íntegro (`PRAGMA integrity_check`) e do BarERP.
pub async fn validate_backup_file(path: &Path) -> Result<(), BackupError> {
    let shown = path.display();
    let meta = std::fs::metadata(path)
        .map_err(|_| msg(format!("Arquivo de backup não encontrado: {shown}")))?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(msg(format!(
            "O arquivo de backup está vazio ou não é um arquivo: {shown}"
        )));
    }
    let mut conn = open(path, true).await.map_err(|e| {
        msg(format!(
            "O arquivo não é um banco de dados válido ({shown}): {e}"
        ))
    })?;
    let check = async {
        let rows = sqlx::query("PRAGMA integrity_check")
            .fetch_all(&mut conn)
            .await?;
        let lines: Vec<String> = rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>(0).ok())
            .collect();
        if lines != ["ok"] {
            return Err(msg(format!(
                "O backup está corrompido: {}",
                lines.join("; ")
            )));
        }
        let tables: Vec<String> =
            sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
                .fetch_all(&mut conn)
                .await?
                .iter()
                .filter_map(|r| r.try_get::<String, _>(0).ok())
                .collect();
        let missing: Vec<&str> = REQUIRED_TABLES
            .iter()
            .copied()
            .filter(|t| !tables.iter().any(|n| n == t))
            .collect();
        if !missing.is_empty() {
            return Err(msg(format!(
                "O arquivo não é um backup do BarERP (faltam as tabelas {}).",
                missing.join(", ")
            )));
        }
        Ok(())
    }
    .await;
    let _ = conn.close().await;
    check.map_err(|e| match e {
        BackupError::Db(e) => msg(format!(
            "O arquivo não é um banco de dados válido ({shown}): {e}"
        )),
        other => other,
    })
}

/// Troca o banco em `db_path` pelo backup `backup_file`.
///
/// 1. valida o backup ([`validate_backup_file`]);
/// 2. guarda uma cópia íntegra do banco atual em `<pasta do banco>/backups/antes-da-restauracao_*.db`;
/// 3. copia o backup para `<banco>.restore.tmp`, apaga `-wal`/`-shm`/`-journal` do banco atual
///    (um WAL antigo aplicado sobre o arquivo novo o corromperia) e renomeia por cima;
/// 4. confere o banco restaurado; se falhar, volta a cópia do passo 2.
///
/// **O pool do sistema tem que estar fechado** antes de chamar e ser reaberto depois.
pub async fn restore(db_path: &Path, backup_file: &Path) -> Result<RestoreResult, BackupError> {
    validate_backup_file(backup_file).await?;
    if same_file(db_path, backup_file) {
        return Err(msg(
            "Escolha um arquivo de backup, não o próprio banco de dados em uso.",
        ));
    }

    let base_dir = db_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let backups_dir = base_dir.join("backups");
    tokio::fs::create_dir_all(&backups_dir).await?;

    // Cópia de segurança do banco atual (pelo SQLite, para levar junto o que estiver no WAL)
    let safety_copy = if db_path.exists() {
        let target = backups_dir.join(format!("{SAFETY_PREFIX}{}.db", stamp(Local::now())));
        let tmp = tmp_of(&target);
        remove_if_exists(&tmp).await?;
        let mut conn = open(db_path, false).await.map_err(|e| {
            msg(format!(
                "Não foi possível abrir o banco atual para guardar a cópia de segurança: {e}"
            ))
        })?;
        let copied = vacuum_into(&mut conn, &tmp).await;
        let _ = conn.close().await;
        copied.map_err(|e| {
            msg(format!(
                "Não foi possível guardar a cópia de segurança do banco atual: {e}"
            ))
        })?;
        tokio::fs::rename(&tmp, &target).await?;
        Some(target)
    } else {
        None
    };

    let staged = with_suffix(db_path, ".restore.tmp");
    remove_if_exists(&staged).await?;
    tokio::fs::copy(backup_file, &staged).await?;
    swap_in(db_path, &staged).await?;

    if let Err(e) = validate_backup_file(db_path).await {
        // Não deixa o sistema com um banco ruim: volta como estava
        if let Some(copy) = &safety_copy {
            let back = with_suffix(db_path, ".rollback.tmp");
            tokio::fs::copy(copy, &back).await?;
            swap_in(db_path, &back).await?;
        }
        return Err(msg(format!(
            "A restauração foi desfeita porque o banco restaurado não abriu: {e}"
        )));
    }

    tracing::info!("[Backup] Banco restaurado de {}", backup_file.display());
    Ok(RestoreResult {
        safety_copy,
        restored_from: backup_file.to_path_buf(),
    })
}

/// Apaga os arquivos auxiliares do SQLite e põe `staged` no lugar do banco.
async fn swap_in(db_path: &Path, staged: &Path) -> Result<(), BackupError> {
    for suffix in ["-wal", "-shm", "-journal"] {
        remove_if_exists(&with_suffix(db_path, suffix)).await?;
    }
    tokio::fs::rename(staged, db_path).await.map_err(|e| {
        msg(format!(
            "Não foi possível substituir o banco de dados ({e}). Feche o sistema em outros computadores e tente de novo."
        ))
    })?;
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
