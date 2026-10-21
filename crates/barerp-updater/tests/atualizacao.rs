//! Atualizador contra um GitHub simulado (wiremock): nenhuma chamada ao GitHub de verdade.

use barerp_updater::{check, download, Endpoints, LocalBuild};
use chrono::{TimeZone, Utc};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn endpoints(server: &MockServer) -> Endpoints {
    Endpoints {
        version_json: format!("{}/latest/version.json", server.uri()),
        installer: format!("{}/latest/BarERP-Instalador-Windows.exe", server.uri()),
        releases_atom: format!("{}/releases.atom", server.uri()),
        api_latest: format!("{}/api/latest", server.uri()),
    }
}

fn local(version: &str, commit: &str) -> LocalBuild {
    LocalBuild { version: version.into(), commit: commit.into(), build_time: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap() }
}

#[tokio::test]
async fn version_json_newer_and_same_commit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/latest/version.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "version": "2.1.0", "commit": "abcdef1234", "buildTime": "2026-10-20T10:00:00Z" })))
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/latest/BarERP-Instalador-Windows.exe"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "5000").insert_header("last-modified", "Tue, 20 Oct 2026 10:05:00 GMT"))
        .mount(&server)
        .await;

    let info = check(&endpoints(&server), &local("2.0.0", "local")).await;
    assert!(info.has_update);
    assert_eq!(info.latest_version, "v2.1.0");
    assert_eq!(info.release_name, "BarERP Pro v2.1.0 (abcdef1)");
    assert_eq!(info.asset_date.unwrap().to_rfc3339(), "2026-10-20T10:05:00+00:00");

    let same = check(&endpoints(&server), &local("2.1.0", "abcdef1234")).await;
    assert!(!same.has_update);
    assert_eq!(same.message.as_deref(), Some("Você já está na versão mais recente (v2.1.0)."));

    // Release mais velha que o instalado (ex.: release ainda do Electron 1.9.x): não "atualiza" para trás
    let older = check(&endpoints(&server), &local("3.0.0", "local")).await;
    assert!(!older.has_update);
}

#[tokio::test]
async fn atom_feed_fallback() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-length", "9").insert_header("last-modified", "Tue, 20 Oct 2026 10:05:00 GMT"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/releases.atom"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<feed><entry><title>BarERP Pro v2.0.5</title><updated>2026-10-20T10:00:00Z</updated><content type=\"html\">&lt;p&gt;Caixa novo&lt;/p&gt;</content></entry></feed>",
        ))
        .mount(&server)
        .await;
    let info = check(&endpoints(&server), &local("2.0.0", "local")).await;
    assert!(info.has_update);
    assert_eq!(info.latest_version, "v2.0.5");
    assert_eq!(info.release_notes, "Caixa novo");
    assert_eq!(info.file_size, 9);
}

#[tokio::test]
async fn nothing_reachable() {
    let server = MockServer::start().await;
    let info = check(&endpoints(&server), &local("2.0.0", "local")).await;
    assert!(!info.has_update);
    assert_eq!(info.message.as_deref(), Some("Não foi possível verificar atualizações no momento."));
}

#[tokio::test]
async fn download_with_progress() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/latest/BarERP-Instalador-Windows.exe"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 300_000]))
        .mount(&server)
        .await;
    let dir = std::env::temp_dir().join(format!("barerp-updater-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("BarERP-Instalador-Update.exe");
    let mut last = None;
    let path = download(&endpoints(&server).installer, &target, |p| last = Some(p)).await.unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 300_000);
    assert_eq!(last.unwrap().percent, 100);
    let _ = std::fs::remove_dir_all(&dir);

    let err = download(&format!("{}/nao-existe", server.uri()), &target, |_| {}).await.unwrap_err();
    assert_eq!(err.to_string(), "Falha no download da atualização (Status 404)");
}
