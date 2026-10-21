//! Serviço de backup: um por vez, agendador e status para a tela de Configurações.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Local, Utc};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use tokio::sync::watch;

use crate::files::{
    self, file_info, is_external_backup, is_local_backup, iso, list, prune, BackupFileInfo,
    EXTERNAL_PREFIX, LOCAL_PREFIX,
};
use crate::settings::{self, assert_writable_folder, BackupSettings, BackupSettingsInput};
use crate::{msg, BackupError};

const STARTUP_DELAY: Duration = Duration::from_secs(15);
const SCHEDULE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub success: bool,
    pub reason: String,
    pub local: Option<BackupFileInfo>,
    pub external: Option<BackupFileInfo>,
    pub external_error: Option<String>,
    pub error: Option<String>,
}

/// Resposta de `GET /system/backup` (mesmos campos do Node).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub enabled: bool,
    pub external_folder: String,
    pub keep: u32,
    pub last_local: Option<BackupFileInfo>,
    pub last_external: Option<BackupFileInfo>,
    pub external_reachable: bool,
    pub total_local: usize,
    pub total_external: usize,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BackupSource {
    Local,
    External,
}

/// Backup disponível para restaurar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub path: PathBuf,
    pub file: String,
    pub at: String,
    pub source: BackupSource,
}

#[derive(Default)]
struct State {
    last_error: Option<String>,
    last_error_at: Option<String>,
    /// Backup em andamento: quem chegar durante ele espera o mesmo resultado.
    running: Option<watch::Receiver<Option<BackupResult>>>,
}

struct Inner {
    pool: SqlitePool,
    base_dir: PathBuf,
    state: Mutex<State>,
    scheduler_started: AtomicBool,
}

/// Barato de clonar (tudo atrás de um `Arc`).
#[derive(Clone)]
pub struct BackupService {
    inner: Arc<Inner>,
}

impl BackupService {
    /// `db_path` é o arquivo do banco aberto em `pool`; a pasta dele guarda `backups/` e `backup-settings.json`.
    pub fn new(pool: SqlitePool, db_path: impl AsRef<Path>) -> Self {
        let base_dir = db_path
            .as_ref()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Self::with_base_dir(pool, base_dir)
    }

    /// Com a pasta base informada diretamente.
    pub fn with_base_dir(pool: SqlitePool, base_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                pool,
                base_dir: base_dir.into(),
                state: Mutex::new(State::default()),
                scheduler_started: AtomicBool::new(false),
            }),
        }
    }

    /// Descobre o arquivo pelo próprio banco (`PRAGMA database_list`), como o servidor Node faz.
    pub async fn from_pool(pool: SqlitePool) -> Result<Self, BackupError> {
        let rows = sqlx::query("PRAGMA database_list").fetch_all(&pool).await?;
        let file = rows
            .iter()
            .find(|r| r.try_get::<String, _>("name").is_ok_and(|n| n == "main"))
            .and_then(|r| r.try_get::<String, _>("file").ok())
            .filter(|f| !f.is_empty())
            .ok_or_else(|| msg("Não foi possível localizar o arquivo do banco de dados."))?;
        Ok(Self::new(pool, file))
    }

    pub fn base_dir(&self) -> &Path {
        &self.inner.base_dir
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.inner.base_dir.join("backups")
    }

    pub fn settings(&self) -> BackupSettings {
        settings::load(&self.inner.base_dir)
    }

    pub fn save_settings(
        &self,
        input: &BackupSettingsInput,
    ) -> Result<BackupSettings, BackupError> {
        settings::save(&self.inner.base_dir, input)
    }

    /// Faz um backup agora. Um por vez: chamadas durante um backup em andamento (agendador,
    /// fechamento de caixa, botão) recebem o mesmo resultado. O backup roda numa tarefa própria,
    /// então cancelar quem chamou não deixa o arquivo pela metade.
    pub async fn run(&self, reason: &str) -> BackupResult {
        let mut rx = {
            let mut state = self.inner.state.lock().unwrap();
            match &state.running {
                Some(rx) => rx.clone(),
                None => {
                    let (tx, rx) = watch::channel(None);
                    state.running = Some(rx.clone());
                    let this = self.clone();
                    let reason = reason.to_string();
                    tokio::spawn(async move {
                        let result = this.do_backup(&reason).await;
                        this.inner.state.lock().unwrap().running = None;
                        let _ = tx.send(Some(result));
                    });
                    rx
                }
            }
        };
        let done = rx.wait_for(Option::is_some).await.map(|r| r.clone());
        match done {
            Ok(r) => r.expect("esperou por Some"),
            Err(_) => BackupResult {
                success: false,
                reason: reason.to_string(),
                local: None,
                external: None,
                external_error: None,
                error: Some("Falha ao gerar o backup: a tarefa de backup foi interrompida.".into()),
            },
        }
    }

    async fn do_backup(&self, reason: &str) -> BackupResult {
        let mut result = BackupResult {
            success: false,
            reason: reason.to_string(),
            local: None,
            external: None,
            external_error: None,
            error: None,
        };
        match self.backup_steps(reason, &mut result).await {
            Ok(()) => {
                result.success = result.external_error.is_none();
                if result.success {
                    let mut s = self.inner.state.lock().unwrap();
                    s.last_error = None;
                    s.last_error_at = None;
                }
            }
            Err(e) => {
                let m = format!("Falha ao gerar o backup: {e}");
                result.error = Some(m.clone());
                self.record_error(m);
            }
        }
        result
    }

    async fn backup_steps(
        &self,
        reason: &str,
        result: &mut BackupResult,
    ) -> Result<(), BackupError> {
        let settings = self.settings();
        let backups_dir = self.backups_dir();
        tokio::fs::create_dir_all(&backups_dir).await?;

        let name = files::stamp(Local::now());
        let local_file = format!("{LOCAL_PREFIX}{name}.db");
        let local_path = backups_dir.join(&local_file);
        let local_tmp = tmp_of(&local_path);
        remove_if_exists(&local_tmp).await?;

        // VACUUM INTO gera uma cópia íntegra mesmo com o sistema gravando (inclui o que ainda está no WAL)
        vacuum_into(&self.inner.pool, &local_tmp).await?;
        tokio::fs::rename(&local_tmp, &local_path).await?;
        let keep = settings.keep as usize;
        let dir = backups_dir.clone();
        tokio::task::spawn_blocking(move || prune(&dir, is_local_backup, keep))
            .await
            .ok();
        result.local = file_info(&backups_dir, Some(&local_file));
        tracing::info!("[Backup] Backup local criado ({reason}): {local_file}");

        if settings.enabled && !settings.external_folder.is_empty() {
            let external_dir = PathBuf::from(&settings.external_folder);
            let external_file = format!("{EXTERNAL_PREFIX}{name}.db");
            let copy = {
                let external_dir = external_dir.clone();
                let external_file = external_file.clone();
                let local_path = local_path.clone();
                tokio::task::spawn_blocking(move || -> Result<(), BackupError> {
                    assert_writable_folder(&external_dir)?;
                    let target = external_dir.join(&external_file);
                    // .tmp + rename: o Google Drive/OneDrive não sobe um arquivo pela metade
                    let tmp = tmp_of(&target);
                    std::fs::copy(&local_path, &tmp)?;
                    std::fs::rename(&tmp, &target)?;
                    prune(&external_dir, is_external_backup, keep);
                    Ok(())
                })
                .await
                .unwrap_or_else(|e| Err(msg(e.to_string())))
            };
            match copy {
                Ok(()) => {
                    result.external = file_info(&external_dir, Some(&external_file));
                    tracing::info!(
                        "[Backup] Cópia externa criada: {}",
                        external_dir.join(&external_file).display()
                    );
                }
                Err(e) => {
                    let e = e.to_string();
                    self.record_error(format!(
                        "Backup local salvo, mas a cópia externa falhou: {e}"
                    ));
                    result.external_error = Some(e);
                }
            }
        }
        Ok(())
    }

    fn record_error(&self, message: String) {
        tracing::error!("[Backup] {message}");
        let mut s = self.inner.state.lock().unwrap();
        s.last_error = Some(message);
        s.last_error_at = Some(iso(Utc::now()));
    }

    pub fn status(&self) -> BackupStatus {
        let settings = self.settings();
        let backups_dir = self.backups_dir();
        let local = list(&backups_dir, is_local_backup);
        let external_dir = Path::new(&settings.external_folder);
        let reachable = !settings.external_folder.is_empty() && external_dir.exists();
        let external = if reachable {
            list(external_dir, is_external_backup)
        } else {
            Vec::new()
        };
        let s = self.inner.state.lock().unwrap();
        BackupStatus {
            enabled: settings.enabled,
            external_folder: settings.external_folder.clone(),
            keep: settings.keep,
            last_local: file_info(&backups_dir, local.first().map(String::as_str)),
            last_external: if reachable {
                file_info(external_dir, external.first().map(String::as_str))
            } else {
                None
            },
            external_reachable: reachable,
            total_local: local.len(),
            total_external: external.len(),
            last_error: s.last_error.clone(),
            last_error_at: s.last_error_at.clone(),
            running: s.running.is_some(),
        }
    }

    /// Backups locais e da pasta externa (se acessível), mais novo primeiro — para escolher o que restaurar.
    pub fn available_backups(&self) -> Vec<BackupEntry> {
        let mut out = Vec::new();
        let mut add = |dir: &Path, matches: fn(&str) -> bool, source| {
            for f in list(dir, matches) {
                if let Some(info) = file_info(dir, Some(&f)) {
                    out.push(BackupEntry {
                        path: dir.join(&f),
                        file: info.file,
                        at: info.at,
                        source,
                    });
                }
            }
        };
        add(&self.backups_dir(), is_local_backup, BackupSource::Local);
        let settings = self.settings();
        if !settings.external_folder.is_empty() {
            add(
                Path::new(&settings.external_folder),
                is_external_backup,
                BackupSource::External,
            );
        }
        out.sort_by(|a, b| b.at.cmp(&a.at));
        out
    }

    /// Já existe backup local com a data de hoje?
    pub fn has_backup_today(&self) -> bool {
        let today = format!("{LOCAL_PREFIX}{}", files::day(Local::now()));
        list(&self.backups_dir(), is_local_backup)
            .iter()
            .any(|f| f.starts_with(&today))
    }

    /// Agendador: 15s depois de iniciar faz backup se ainda não houver um de hoje, e depois a cada 6 horas.
    /// Só inicia uma vez por serviço; chamadas seguintes devolvem `None`.
    pub fn spawn_scheduler(&self) -> Option<tokio::task::JoinHandle<()>> {
        self.spawn_scheduler_with(STARTUP_DELAY, SCHEDULE_INTERVAL)
    }

    #[doc(hidden)]
    pub fn spawn_scheduler_with(
        &self,
        startup: Duration,
        every: Duration,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if self.inner.scheduler_started.swap(true, Ordering::SeqCst) {
            return None;
        }
        let this = self.clone();
        // O intervalo conta desde a inicialização, independente do backup de inicialização (como o setInterval)
        let first_tick = tokio::time::Instant::now() + every;
        Some(tokio::spawn(async move {
            let startup_job = async {
                tokio::time::sleep(startup).await;
                if !this.has_backup_today() {
                    this.run("inicialização").await;
                }
            };
            let schedule = async {
                let mut ticker = tokio::time::interval_at(first_tick, every);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    this.run("agendado (6h)").await;
                }
            };
            tokio::join!(startup_job, schedule);
        }))
    }
}

pub(crate) fn tmp_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

pub(crate) async fn remove_if_exists(path: &Path) -> Result<(), BackupError> {
    match tokio::fs::remove_file(path).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// `VACUUM INTO '<arquivo>'` com aspas escapadas.
pub(crate) async fn vacuum_into<'e, E>(executor: E, target: &Path) -> Result<(), BackupError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let target = target
        .to_str()
        .ok_or_else(|| msg("Caminho do backup com caracteres inválidos."))?;
    let sql = format!("VACUUM INTO '{}'", target.replace('\'', "''"));
    sqlx::query(&sql).execute(executor).await?;
    Ok(())
}
