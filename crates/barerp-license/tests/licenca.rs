//! Licença contra um painel simulado (wiremock): nenhuma chamada ao Render de verdade.

use std::time::Duration;

use barerp_db::Db;
use barerp_license::service::{background_check_with_timeout, sync_remote_with_timeout};
use barerp_license::{activate_offline, status};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// sha256("3f2504e0-4f89-11d3-9a0c-0305e82c3301") e chave gerada com o código do Node
const MID: &str = "d1bfaf4aff653cb27984b7d978e51a7d406d1572df95d205c254beb18dc134d3";
const KEY_2099: &str = "EXP-20991231-D1BFAF4A-6B75CA1E";
const T: Duration = Duration::from_secs(3);

async fn exec(db: &Db, sql: &str) {
    sqlx::query(sql).execute(db.pool()).await.unwrap();
}

#[tokio::test]
async fn new_install_is_blocked_until_activation() {
    let db = Db::open_in_memory().await.unwrap();
    let st = status(&db, MID).await.unwrap();
    assert!(!st.is_licensed);
    assert_eq!(st.license_status, "UNLICENSED");
    assert!(st.blocked());
    assert_eq!(st.restaurant_name, "Meu Bar");
    assert!(st.should_check_remote);

    let msg = activate_offline(&db, MID, KEY_2099).await.unwrap();
    assert!(msg.starts_with("Licença offline ativada com sucesso! Válida até 31/12/2099"), "{msg}");
    let st = status(&db, MID).await.unwrap();
    assert!(st.is_licensed && !st.blocked());
    assert_eq!(st.license_status, "ACTIVE");
    assert!(st.days_remaining > 1000);
    assert!(!st.should_check_remote);
}

#[tokio::test]
async fn offline_key_of_other_machine_is_refused() {
    let db = Db::open_in_memory().await.unwrap();
    let err = activate_offline(&db, "outra-maquina", KEY_2099).await.unwrap_err().to_string();
    assert!(err.contains("outro computador"), "{err}");
    assert_eq!(activate_offline(&db, MID, "  ").await.unwrap_err().to_string(), "Chave de ativação é obrigatória.");
}

#[tokio::test]
async fn expired_license_is_marked_and_blocked() {
    let db = Db::open_in_memory().await.unwrap();
    status(&db, MID).await.unwrap();
    exec(&db, r#"UPDATE "SystemSettings" SET "isLicensed" = 1, "licenseStatus" = 'ACTIVE', "expiresAt" = 1000"#).await;
    let st = status(&db, MID).await.unwrap();
    assert!(!st.is_licensed && st.blocked());
    assert_eq!(st.license_status, "EXPIRED");
    assert_eq!(st.days_remaining, 0);
}

#[tokio::test]
async fn expiring_soon_within_five_days() {
    let db = Db::open_in_memory().await.unwrap();
    status(&db, MID).await.unwrap();
    let in_3_days = chrono::Utc::now().timestamp_millis() + 3 * 86_400_000 - 60_000;
    exec(&db, &format!(r#"UPDATE "SystemSettings" SET "isLicensed" = 1, "licenseStatus" = 'ACTIVE', "expiresAt" = {in_3_days}"#)).await;
    let st = status(&db, MID).await.unwrap();
    assert!(st.is_licensed && st.is_expiring_soon);
    assert_eq!(st.days_remaining, 3);
}

#[tokio::test]
async fn legacy_key_checked_against_machine() {
    let db = Db::open_in_memory().await.unwrap();
    status(&db, MID).await.unwrap();
    exec(
        &db,
        r#"UPDATE "SystemSettings" SET "isLicensed" = 1, "licenseStatus" = 'ACTIVE', "licenseEmail" = 'dono@bar.com',
             "licenseKey" = '18e54c7afd34150b46deba1f27253f33fbe904be4b55dd636c971d47f49a1e6a'"#,
    )
    .await;
    assert!(!status(&db, MID).await.unwrap().blocked());
    // Mesmo banco em outro computador: a chave legada não vale
    let st = status(&db, "outro").await.unwrap();
    assert_eq!(st.license_status, "INVALID");
    assert!(st.blocked());
}

#[tokio::test]
async fn sync_remote_activates_from_panel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/licenses/check/{MID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "valid": true, "status": "ACTIVE", "expiresAt": "2099-01-31T23:59:59.000Z",
            "clientName": "JUBAR", "daysRemaining": 30,
            "developerContact": { "phone": "5547974002560", "pixKey": "pix-novo" }
        })))
        .mount(&server)
        .await;

    let db = Db::open_in_memory().await.unwrap();
    let out = sync_remote_with_timeout(&db, MID, Some(&server.uri()), T).await.unwrap();
    assert!(out.activated);
    assert_eq!(out.message, "Licença ativada e regularizada com sucesso!");
    let st = status(&db, MID).await.unwrap();
    assert!(!st.blocked());
    assert_eq!(st.client_name.as_deref(), Some("JUBAR"));
    assert_eq!(st.developer_contact.pix_key, "pix-novo");
    assert_eq!(st.license_server_url.as_deref(), Some(server.uri().as_str()));
}

#[tokio::test]
async fn sync_remote_blocked_by_panel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "success": true, "valid": false, "status": "BLOCKED", "message": "Terminal suspenso."
        })))
        .mount(&server)
        .await;
    let db = Db::open_in_memory().await.unwrap();
    activate_offline(&db, MID, KEY_2099).await.unwrap();
    let err = sync_remote_with_timeout(&db, MID, Some(&server.uri()), T).await.unwrap_err().to_string();
    assert_eq!(err, "Terminal suspenso.");
    let st = status(&db, MID).await.unwrap();
    assert_eq!(st.license_status, "BLOCKED");
    assert!(st.blocked());
}

#[tokio::test]
async fn sync_remote_offline_keeps_valid_local_license() {
    let db = Db::open_in_memory().await.unwrap();
    // Porta fechada: falha de conexão
    let err = sync_remote_with_timeout(&db, MID, Some("http://127.0.0.1:9"), T).await.unwrap_err().to_string();
    assert!(err.starts_with("Não foi possível conectar ao servidor de licenças"), "{err}");

    activate_offline(&db, MID, KEY_2099).await.unwrap();
    let out = sync_remote_with_timeout(&db, MID, Some("http://127.0.0.1:9"), T).await.unwrap();
    assert!(out.offline_fallback && !out.activated);
    assert!(out.message.starts_with("Servidor remoto inacessível, mas sua licença local continua ativa até 31/12/2099"), "{}", out.message);
}

#[tokio::test]
async fn background_check_blocks_and_renews() {
    let server = MockServer::start().await;
    let db = Db::open_in_memory().await.unwrap();
    activate_offline(&db, MID, KEY_2099).await.unwrap();
    exec(&db, &format!(r#"UPDATE "SystemSettings" SET "licenseServerUrl" = '{}'"#, server.uri())).await;

    // Resposta não 2xx é ignorada em segundo plano
    let blocked = Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "success": true, "valid": false, "status": "BLOCKED" })))
        .mount_as_scoped(&server)
        .await;
    background_check_with_timeout(&db, MID, T).await.unwrap();
    assert!(!status(&db, MID).await.unwrap().blocked());
    drop(blocked);

    let blocked = Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true, "valid": false, "status": "BLOCKED" })))
        .mount_as_scoped(&server)
        .await;
    background_check_with_timeout(&db, MID, T).await.unwrap();
    assert_eq!(status(&db, MID).await.unwrap().license_status, "BLOCKED");
    drop(blocked);

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "success": true, "valid": true, "expiresAt": "2099-06-30T23:59:59Z" })))
        .mount(&server)
        .await;
    background_check_with_timeout(&db, MID, T).await.unwrap();
    let st = status(&db, MID).await.unwrap();
    assert!(!st.blocked());
    assert_eq!(st.expires_at.unwrap().to_rfc3339(), "2099-06-30T23:59:59+00:00");
}

#[tokio::test]
async fn legacy_email_key_activation() {
    use barerp_license::service::activate_legacy;
    let db = Db::open_in_memory().await.unwrap();
    assert!(!activate_legacy(&db, MID, "dono@bar.com", "errada").await.unwrap());
    assert!(status(&db, MID).await.unwrap().blocked());
    // Chave gerada com o código do Node para este e-mail e esta máquina
    let key = "18e54c7afd34150b46deba1f27253f33fbe904be4b55dd636c971d47f49a1e6a";
    assert!(activate_legacy(&db, MID, " dono@bar.com ", key).await.unwrap());
    let st = status(&db, MID).await.unwrap();
    assert!(st.is_licensed && !st.blocked());
}
