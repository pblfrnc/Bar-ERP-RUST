use std::path::{Path, PathBuf};

use barerp_backup::{
    restore, validate_backup_file, BackupError, BackupService, BackupSettingsInput,
};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

/// Banco em WAL com as tabelas mínimas do BarERP, como o sistema abre.
async fn open_db(path: &Path) -> SqlitePool {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .unwrap()
}

async fn new_db(dir: &Path) -> (PathBuf, SqlitePool) {
    let path = dir.join("bar.db");
    let pool = open_db(&path).await;
    for sql in [
        r#"CREATE TABLE "Order" (id TEXT PRIMARY KEY, total REAL)"#,
        r#"CREATE TABLE "Product" (id TEXT PRIMARY KEY, name TEXT)"#,
        r#"CREATE TABLE "SystemSettings" (id TEXT PRIMARY KEY)"#,
        r#"INSERT INTO "Product" VALUES ('p1', 'Cerveja')"#,
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    (path, pool)
}

async fn product_names(pool: &SqlitePool) -> Vec<String> {
    sqlx::query(r#"SELECT name FROM "Product" ORDER BY id"#)
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect()
}

fn files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|r| {
            r.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[tokio::test]
async fn backup_local_com_dados_do_wal() {
    let dir = tempfile::tempdir().unwrap();
    let (_, pool) = new_db(dir.path()).await;
    let svc = BackupService::from_pool(pool.clone()).await.unwrap();
    assert_eq!(
        svc.base_dir().canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );
    assert!(!svc.has_backup_today());

    let r = svc.run("manual").await;
    assert!(r.success, "{r:?}");
    assert_eq!(r.reason, "manual");
    let local = r.local.unwrap();
    assert!(local.file.starts_with("backup_") && local.file.ends_with(".db"));
    assert!(local.at.ends_with('Z'));
    assert!(svc.has_backup_today());
    // Sem .tmp sobrando
    assert_eq!(files(&svc.backups_dir()), vec![local.file.clone()]);

    // O backup tem o que a pool gravou (mesmo antes do checkpoint do WAL)
    let backup = open_db(&svc.backups_dir().join(&local.file)).await;
    assert_eq!(product_names(&backup).await, vec!["Cerveja"]);
    validate_backup_file(&svc.backups_dir().join(&local.file))
        .await
        .unwrap();

    let st = svc.status();
    assert_eq!(st.total_local, 1);
    assert_eq!(st.last_local.unwrap().file, local.file);
    assert!(!st.running && st.last_error.is_none());
}

#[tokio::test]
async fn copia_externa_e_falha_quando_a_pasta_some() {
    let dir = tempfile::tempdir().unwrap();
    let ext = tempfile::tempdir().unwrap();
    let (db, pool) = new_db(dir.path()).await;
    let svc = BackupService::new(pool, &db);

    let input = BackupSettingsInput {
        external_folder: Some(format!("  {}  ", ext.path().display())),
        enabled: Some(true),
        keep: Some(json!("7")),
    };
    let saved = svc.save_settings(&input).unwrap();
    assert_eq!(saved.keep, 7);
    assert_eq!(saved.external_folder, ext.path().display().to_string());
    // Mesmo formato do Node: JSON com 2 espaços e as mesmas chaves
    let text = std::fs::read_to_string(dir.path().join("backup-settings.json")).unwrap();
    assert!(text.starts_with("{\n  \"externalFolder\": "), "{text}");
    assert!(text.contains("\"enabled\": true") && text.contains("\"keep\": 7"));

    let r = svc.run("fechamento de caixa").await;
    assert!(r.success, "{r:?}");
    let external = r.external.unwrap();
    assert!(external.file.starts_with("bar-erp-backup_"));
    assert_eq!(files(ext.path()), vec![external.file.clone()]);
    assert_eq!(svc.status().total_external, 1);
    assert_eq!(svc.available_backups().len(), 2);

    // Pen drive removido: backup local continua, cópia externa vira erro
    let ext_path = ext.path().to_path_buf();
    drop(ext);
    let r = svc.run("agendado (6h)").await;
    assert!(!r.success);
    assert!(r.local.is_some());
    assert!(r.external_error.unwrap().contains("não foi encontrada"));
    let st = svc.status();
    assert!(!st.external_reachable);
    assert!(st
        .last_error
        .unwrap()
        .starts_with("Backup local salvo, mas a cópia externa falhou"));
    assert!(st.last_error_at.is_some());
    assert!(!ext_path.exists());
}

#[tokio::test]
async fn validacoes_da_configuracao() {
    let dir = tempfile::tempdir().unwrap();
    let (db, pool) = new_db(dir.path()).await;
    let svc = BackupService::new(pool, &db);
    let err = |input: BackupSettingsInput| svc.save_settings(&input).unwrap_err().to_string();

    assert!(err(BackupSettingsInput {
        enabled: Some(true),
        ..Default::default()
    })
    .contains("Escolha a pasta"));
    assert!(err(BackupSettingsInput {
        external_folder: Some("relativa/pasta".into()),
        ..Default::default()
    })
    .contains("caminho completo"));
    let inside = dir.path().join("backups").display().to_string();
    assert!(err(BackupSettingsInput {
        external_folder: Some(inside),
        ..Default::default()
    })
    .contains("dentro da pasta"));

    // Desligado, a pasta pode não existir (pen drive fora)
    let missing = dir
        .path()
        .parent()
        .unwrap()
        .join("pen-drive-que-nao-existe")
        .display()
        .to_string();
    let ok = svc
        .save_settings(&BackupSettingsInput {
            external_folder: Some(missing.clone()),
            enabled: Some(false),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ok.external_folder, missing);
    assert_eq!(svc.settings().keep, 30);

    // Arquivo com tipos errados: cada campo cai no padrão
    std::fs::write(
        dir.path().join("backup-settings.json"),
        r#"{"externalFolder": 5, "enabled": "sim", "keep": "12"}"#,
    )
    .unwrap();
    let s = svc.settings();
    assert_eq!(
        (s.external_folder.as_str(), s.enabled, s.keep),
        ("", false, 12)
    );
}

#[tokio::test]
async fn chamadas_simultaneas_fazem_um_backup_so() {
    let dir = tempfile::tempdir().unwrap();
    let (db, pool) = new_db(dir.path()).await;
    let svc = BackupService::new(pool, &db);
    let (a, b, c) = tokio::join!(
        svc.run("manual"),
        svc.run("fechamento de caixa"),
        svc.run("agendado (6h)")
    );
    assert_eq!(a, b);
    assert_eq!(b, c);
    assert_eq!(files(&svc.backups_dir()).len(), 1);
}

#[tokio::test]
async fn agendador_faz_backup_de_inicializacao_uma_vez() {
    let dir = tempfile::tempdir().unwrap();
    let (db, pool) = new_db(dir.path()).await;
    let svc = BackupService::new(pool, &db);
    let handle = svc
        .spawn_scheduler_with(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(3600),
        )
        .unwrap();
    assert!(
        svc.spawn_scheduler().is_none(),
        "só um agendador por serviço"
    );
    for _ in 0..200 {
        if svc.has_backup_today() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(svc.has_backup_today());
    handle.abort();
}

#[tokio::test]
async fn restaura_backup_e_guarda_copia_do_atual() {
    let dir = tempfile::tempdir().unwrap();
    let (db, pool) = new_db(dir.path()).await;
    let svc = BackupService::new(pool.clone(), &db);
    let backup = svc
        .backups_dir()
        .join(svc.run("manual").await.local.unwrap().file);

    // Depois do backup o banco muda
    sqlx::query(r#"UPDATE "Product" SET name = 'Chopp'"#)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(product_names(&pool).await, vec!["Chopp"]);

    // Quem chama fecha o pool antes de restaurar
    pool.close().await;
    let r = restore(&db, &backup).await.unwrap();
    let safety = r.safety_copy.unwrap();
    assert!(safety
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("antes-da-restauracao_"));

    let pool = open_db(&db).await;
    assert_eq!(product_names(&pool).await, vec!["Cerveja"]);
    // A cópia de segurança tem o banco como estava antes da troca
    assert_eq!(product_names(&open_db(&safety).await).await, vec!["Chopp"]);
    // E não entra na retenção nem na lista de backups
    let svc = BackupService::new(pool, &db);
    assert_eq!(svc.status().total_local, 1);
}

#[tokio::test]
async fn recusa_arquivo_corrompido_ou_de_outro_sistema() {
    let dir = tempfile::tempdir().unwrap();
    let (db, pool) = new_db(dir.path()).await;
    pool.close().await;

    let garbage = dir.path().join("lixo.db");
    std::fs::write(&garbage, b"isto nao e um banco sqlite").unwrap();
    let e = restore(&db, &garbage).await.unwrap_err();
    assert!(
        matches!(e, BackupError::Message(ref m) if m.contains("não é um banco de dados válido")),
        "{e}"
    );

    let other = dir.path().join("outro.db");
    let p = open_db(&other).await;
    sqlx::query("CREATE TABLE clientes (id INTEGER)")
        .execute(&p)
        .await
        .unwrap();
    p.close().await;
    let e = restore(&db, &other).await.unwrap_err().to_string();
    assert!(e.contains("não é um backup do BarERP"), "{e}");

    let e = restore(&db, &dir.path().join("nao-existe.db"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("não encontrado"), "{e}");

    // Nada foi trocado
    assert_eq!(product_names(&open_db(&db).await).await, vec!["Cerveja"]);
}
