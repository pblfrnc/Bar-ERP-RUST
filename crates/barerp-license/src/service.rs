//! Status da licença e ativação, gravando na linha `default` da tabela `SystemSettings` (mesmas colunas do Prisma).

use std::time::Duration;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::FromRow;

use barerp_db::{Db, DbTime};

use crate::keys::{br_date, legacy_key, verify_offline_key};
use crate::remote::{self, CheckError};
use crate::{secret_key, LicenseError, LicenseResult, DEFAULT_SERVER_URL};

const DAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

#[derive(Debug, Clone, FromRow)]
struct Row {
    #[sqlx(rename = "restaurantName")]
    restaurant_name: String,
    #[sqlx(rename = "licenseEmail")]
    license_email: Option<String>,
    #[sqlx(rename = "licenseKey")]
    license_key: Option<String>,
    #[sqlx(rename = "machineId")]
    machine_id: Option<String>,
    #[sqlx(rename = "isLicensed")]
    is_licensed: bool,
    #[sqlx(rename = "defaultProfitMargin")]
    default_profit_margin: Option<f64>,
    #[sqlx(rename = "expiresAt")]
    expires_at: Option<DbTime>,
    #[sqlx(rename = "lastVerifiedAt")]
    last_verified_at: Option<DbTime>,
    #[sqlx(rename = "clientName")]
    client_name: Option<String>,
    #[sqlx(rename = "licenseStatus")]
    license_status: Option<String>,
    #[sqlx(rename = "licenseServerUrl")]
    license_server_url: Option<String>,
    #[sqlx(rename = "developerContact")]
    developer_contact: Option<String>,
}

/// Contato do desenvolvedor para renovação (PIX e WhatsApp).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperContact {
    pub phone: String,
    pub pix_key: String,
    pub developer_name: String,
}

impl Default for DeveloperContact {
    fn default() -> Self {
        Self {
            phone: "5547974002560".into(),
            pix_key: "68.817.608/0001-47".into(),
            developer_name: "Pablo Franco - Software House".into(),
        }
    }
}

impl DeveloperContact {
    /// Padrão + o que o painel mandou (o número provisório antigo é trocado pelo atual, como no Node).
    fn from_json(raw: Option<&str>) -> Self {
        let mut c = Self::default();
        let Some(Value::Object(map)) = raw.and_then(|r| serde_json::from_str::<Value>(r).ok()) else { return c };
        let text = |k: &str| map.get(k).and_then(Value::as_str).map(str::to_string);
        if let Some(phone) = text("phone") {
            c.phone = if phone.contains("91988887777") || phone.contains("988887777") { "5547974002560".into() } else { phone };
        }
        if let Some(pix) = text("pixKey") {
            c.pix_key = pix;
        }
        if let Some(name) = text("developerName") {
            c.developer_name = name;
        }
        c
    }

    /// Telefone com DDI 55 para o link do WhatsApp.
    pub fn whatsapp_phone(&self) -> String {
        let digits: String = self.phone.chars().filter(char::is_ascii_digit).collect();
        if digits.starts_with("55") { digits } else { format!("55{digits}") }
    }
}

/// O que o GET `/api/settings` devolvia (sem a chave secreta).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStatus {
    pub machine_id: String,
    pub restaurant_name: String,
    pub is_licensed: bool,
    pub license_status: String,
    pub days_remaining: i64,
    pub is_expiring_soon: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub client_name: Option<String>,
    pub license_server_url: Option<String>,
    pub default_profit_margin: f64,
    pub developer_contact: DeveloperContact,
    /// Última verificação no painel há mais de 2 minutos: quem chamou deve rodar [`background_check`]
    #[serde(skip)]
    pub should_check_remote: bool,
}

impl LicenseStatus {
    /// Bloqueio total do App.tsx: sem licença, expirada, bloqueada ou inválida → tela de ativação.
    pub fn blocked(&self) -> bool {
        !self.is_licensed || matches!(self.license_status.as_str(), "BLOCKED" | "EXPIRED" | "UNLICENSED" | "INVALID")
    }
}

const SELECT: &str = r#"SELECT "restaurantName", "licenseEmail", "licenseKey", "machineId", "isLicensed", "defaultProfitMargin",
       "expiresAt", "lastVerifiedAt", "clientName", "licenseStatus", "licenseServerUrl", "developerContact"
FROM "SystemSettings" WHERE "id" = 'default'"#;

async fn load(db: &Db) -> LicenseResult<Option<Row>> {
    Ok(sqlx::query_as::<_, Row>(SELECT).fetch_optional(db.pool()).await?)
}

/// Linha `default` (criada como no Node se ainda não existe), com o `machineId` desta máquina.
async fn ensure_row(db: &Db, machine_id: &str) -> LicenseResult<Row> {
    if let Some(row) = load(db).await? {
        if row.machine_id.as_deref() != Some(machine_id) {
            let mut tx = db.begin_write().await?;
            sqlx::query(r#"UPDATE "SystemSettings" SET "machineId" = ?, "updatedAt" = ? WHERE "id" = 'default'"#)
                .bind(machine_id)
                .bind(DbTime::now())
                .execute(&mut **tx)
                .await?;
            tx.commit().await?;
            return Ok(Row { machine_id: Some(machine_id.to_string()), ..row });
        }
        return Ok(row);
    }
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"INSERT OR IGNORE INTO "SystemSettings" ("id", "restaurantName", "machineId", "isLicensed", "licenseStatus", "updatedAt")
           VALUES ('default', 'Meu Bar', ?, 0, 'UNLICENSED', ?)"#,
    )
    .bind(machine_id)
    .bind(DbTime::now())
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    load(db).await?.ok_or_else(|| LicenseError::msg("Erro ao buscar configurações"))
}

async fn set_state(db: &Db, is_licensed: bool, status: &str) -> LicenseResult<()> {
    let mut tx = db.begin_write().await?;
    sqlx::query(r#"UPDATE "SystemSettings" SET "isLicensed" = ?, "licenseStatus" = ?, "updatedAt" = ? WHERE "id" = 'default'"#)
        .bind(is_licensed)
        .bind(status)
        .bind(DbTime::now())
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

fn days_until(at: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    ((at - now).num_milliseconds() as f64 / DAY_MS).ceil() as i64
}

/// Status da licença, com as mesmas regras do GET `/api/settings`: expiração, chave legada e bloqueio pelo painel.
pub async fn status(db: &Db, machine_id: &str) -> LicenseResult<LicenseStatus> {
    let mut row = ensure_row(db, machine_id).await?;
    let now = Utc::now();
    let current_status = row.license_status.clone().unwrap_or_default();

    let mut is_licensed = row.is_licensed;
    if matches!(current_status.as_str(), "BLOCKED" | "INVALID" | "EXPIRED") {
        is_licensed = false;
    }
    let mut days_remaining = 0;
    let mut is_expiring_soon = false;

    if let Some(exp) = row.expires_at.map(|t| t.0) {
        days_remaining = days_until(exp, now).max(0);
        if now > exp {
            is_licensed = false;
            if row.is_licensed || current_status != "EXPIRED" {
                set_state(db, false, "EXPIRED").await?;
                row.is_licensed = false;
                row.license_status = Some("EXPIRED".into());
            }
        } else if days_remaining <= 5 && is_licensed {
            is_expiring_soon = true;
        }
    } else if is_licensed && row.license_key.is_none() {
        is_licensed = false;
        set_state(db, false, "UNLICENSED").await?;
        row.is_licensed = false;
        row.license_status = Some("UNLICENSED".into());
    } else if row.is_licensed {
        if let (Some(email), Some(key)) = (&row.license_email, &row.license_key) {
            if legacy_key(&secret_key(), email, machine_id) != *key {
                is_licensed = false;
                set_state(db, false, "INVALID").await?;
                row.is_licensed = false;
                row.license_status = Some("INVALID".into());
            }
        }
    }

    let should_check_remote = row.last_verified_at.is_none_or(|t| (now - t.0).num_milliseconds() > 2 * 60 * 1000);
    let license_status = row
        .license_status
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| if is_licensed { "ACTIVE".into() } else { "UNLICENSED".into() });

    Ok(LicenseStatus {
        machine_id: machine_id.to_string(),
        restaurant_name: row.restaurant_name,
        is_licensed,
        license_status,
        days_remaining,
        is_expiring_soon,
        expires_at: row.expires_at.map(|t| t.0),
        last_verified_at: row.last_verified_at.map(|t| t.0),
        client_name: row.client_name,
        license_server_url: row.license_server_url,
        default_profit_margin: row.default_profit_margin.unwrap_or(50.0),
        developer_contact: DeveloperContact::from_json(row.developer_contact.as_deref()),
        should_check_remote,
    })
}

fn server_url(override_url: Option<&str>, row: &Row) -> String {
    override_url
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("LICENSE_SERVER_URL").ok().filter(|u| !u.trim().is_empty()))
        .or_else(|| row.license_server_url.clone().filter(|u| !u.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string())
}

/// `new Date(data.expiresAt)`: ISO completo ou só a data.
fn parse_expires(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim())
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| Utc.from_utc_datetime(&dt))
        })
}

/// Resultado do "Verificar Ativação Online Agora".
#[derive(Debug, Clone, PartialEq)]
pub struct SyncOutcome {
    /// `true`: o painel liberou (ativou ou renovou) esta máquina
    pub activated: bool,
    /// Painel fora do ar, mas a licença local ainda vale
    pub offline_fallback: bool,
    pub message: String,
    pub days_remaining: Option<i64>,
}

/// Sincroniza com o painel (POST `/license/sync-remote`). `Err` traz a mensagem para a tela.
pub async fn sync_remote(db: &Db, machine_id: &str, override_url: Option<&str>) -> LicenseResult<SyncOutcome> {
    sync_remote_with_timeout(db, machine_id, override_url, Duration::from_secs(6)).await
}

pub async fn sync_remote_with_timeout(
    db: &Db,
    machine_id: &str,
    override_url: Option<&str>,
    timeout: Duration,
) -> LicenseResult<SyncOutcome> {
    let row = ensure_row(db, machine_id).await?;
    let url = server_url(override_url, &row);
    tracing::info!("[Licensing] Sincronizando licença com: {}", remote::check_url(&url, machine_id));

    let checked = match remote::check(&url, machine_id, timeout).await {
        Ok(c) => c,
        Err(CheckError::Network(e)) => {
            tracing::warn!("[Licensing] Falha ao conectar no servidor remoto: {e}");
            let now = Utc::now();
            if let Some(exp) = row.expires_at.map(|t| t.0).filter(|exp| row.is_licensed && *exp > now) {
                let days = days_until(exp, now);
                return Ok(SyncOutcome {
                    activated: false,
                    offline_fallback: true,
                    message: format!(
                        "Servidor remoto inacessível, mas sua licença local continua ativa até {} ({days} dias restantes).",
                        br_date(exp)
                    ),
                    days_remaining: Some(days),
                });
            }
            return Err(LicenseError::msg(
                "Não foi possível conectar ao servidor de licenças. Verifique sua conexão à internet ou use uma chave de ativação offline.",
            ));
        }
        Err(CheckError::BadResponse(msg)) => return Err(LicenseError::msg(msg)),
    };

    let data = checked.body;
    let contact = data.developer_contact.as_ref().map(|v| v.to_string());
    let expires = data.expires_at.as_deref().and_then(parse_expires);
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;
    if data.success && data.valid && expires.is_some() {
        sqlx::query(
            r#"UPDATE "SystemSettings" SET "isLicensed" = 1, "machineId" = ?, "expiresAt" = ?, "clientName" = ?,
                 "licenseStatus" = 'ACTIVE', "licenseServerUrl" = ?, "lastVerifiedAt" = ?,
                 "developerContact" = COALESCE(?, "developerContact"), "updatedAt" = ?
               WHERE "id" = 'default'"#,
        )
        .bind(machine_id)
        .bind(expires.map(DbTime))
        .bind(data.client_name.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "Cliente".into()))
        .bind(&url)
        .bind(now)
        .bind(&contact)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        tx.commit().await?;
        return Ok(SyncOutcome {
            activated: true,
            offline_fallback: false,
            message: "Licença ativada e regularizada com sucesso!".into(),
            days_remaining: data.days_remaining,
        });
    }

    let status = data.status.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "EXPIRED".into());
    sqlx::query(
        r#"UPDATE "SystemSettings" SET "isLicensed" = 0, "licenseStatus" = ?, "lastVerifiedAt" = ?,
             "developerContact" = COALESCE(?, "developerContact"), "updatedAt" = ?
           WHERE "id" = 'default'"#,
    )
    .bind(&status)
    .bind(now)
    .bind(&contact)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Err(LicenseError::msg(
        data.message.filter(|m| !m.is_empty()).unwrap_or_else(|| "Máquina não licenciada ou mensalidade expirada.".into()),
    ))
}

/// Revalidação silenciosa com o painel (a cada 2 min no Node). Falha de rede mantém o estado local.
pub async fn background_check(db: &Db, machine_id: &str) -> LicenseResult<()> {
    background_check_with_timeout(db, machine_id, Duration::from_secs(4)).await
}

pub async fn background_check_with_timeout(db: &Db, machine_id: &str, timeout: Duration) -> LicenseResult<()> {
    let row = ensure_row(db, machine_id).await?;
    let url = server_url(None, &row);
    let Ok(checked) = remote::check(&url, machine_id, timeout).await else { return Ok(()) };
    if !checked.ok || !checked.body.success {
        return Ok(());
    }
    let data = checked.body;
    let now = DbTime::now();
    if data.status.as_deref() == Some("BLOCKED") || !data.valid {
        let status = data.status.filter(|s| !s.is_empty()).unwrap_or_else(|| "EXPIRED".into());
        let mut tx = db.begin_write().await?;
        sqlx::query(
            r#"UPDATE "SystemSettings" SET "isLicensed" = 0, "licenseStatus" = ?, "lastVerifiedAt" = ?, "updatedAt" = ?
               WHERE "id" = 'default'"#,
        )
        .bind(status)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        tx.commit().await?;
    } else if let Some(exp) = data.expires_at.as_deref().and_then(parse_expires) {
        let mut tx = db.begin_write().await?;
        sqlx::query(
            r#"UPDATE "SystemSettings" SET "isLicensed" = 1, "expiresAt" = ?, "licenseStatus" = 'ACTIVE', "lastVerifiedAt" = ?,
                 "updatedAt" = ?
               WHERE "id" = 'default'"#,
        )
        .bind(DbTime(exp))
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Ativa com a chave offline recebida no WhatsApp (POST `/license/offline`). Devolve a mensagem de sucesso.
pub async fn activate_offline(db: &Db, machine_id: &str, key: &str) -> LicenseResult<String> {
    if key.trim().is_empty() {
        return Err(LicenseError::msg("Chave de ativação é obrigatória."));
    }
    ensure_row(db, machine_id).await?;
    let now = Utc::now();
    let expires = verify_offline_key(&secret_key(), key, machine_id, now).map_err(LicenseError::Message)?;
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"UPDATE "SystemSettings" SET "isLicensed" = 1, "machineId" = ?, "expiresAt" = ?, "licenseKey" = ?,
             "licenseStatus" = 'ACTIVE', "lastVerifiedAt" = ?, "updatedAt" = ?
           WHERE "id" = 'default'"#,
    )
    .bind(machine_id)
    .bind(DbTime(expires))
    .bind(key.trim())
    .bind(DbTime(now))
    .bind(DbTime(now))
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    let days = days_until(expires, now);
    Ok(format!("Licença offline ativada com sucesso! Válida até {} ({days} dias).", br_date(expires)))
}

/// Ativação legada por e-mail + chave HMAC permanente (POST `/license`). `Ok(false)` = chave não confere.
pub async fn activate_legacy(db: &Db, machine_id: &str, email: &str, key: &str) -> LicenseResult<bool> {
    ensure_row(db, machine_id).await?;
    if crate::keys::legacy_key(&secret_key(), email.trim(), machine_id) != key.trim() {
        return Ok(false);
    }
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"UPDATE "SystemSettings" SET "licenseEmail" = ?, "licenseKey" = ?, "machineId" = ?, "isLicensed" = 1,
             "licenseStatus" = 'ACTIVE', "updatedAt" = ?
           WHERE "id" = 'default'"#,
    )
    .bind(email.trim())
    .bind(key.trim())
    .bind(machine_id)
    .bind(DbTime::now())
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_merges_and_fixes_old_phone() {
        let c = DeveloperContact::from_json(Some(r#"{"phone":"5591988887777","pixKey":"chave@pix"}"#));
        assert_eq!(c.phone, "5547974002560");
        assert_eq!(c.pix_key, "chave@pix");
        assert_eq!(c.developer_name, "Pablo Franco - Software House");
        assert_eq!(DeveloperContact::from_json(Some("lixo")), DeveloperContact::default());
        assert_eq!(DeveloperContact { phone: "(47) 97400-2560".into(), ..Default::default() }.whatsapp_phone(), "5547974002560");
    }

    #[test]
    fn expires_formats() {
        assert_eq!(parse_expires("2026-11-30T23:59:59.000Z").unwrap().to_rfc3339(), "2026-11-30T23:59:59+00:00");
        assert_eq!(parse_expires("2026-11-30").unwrap().to_rfc3339(), "2026-11-30T00:00:00+00:00");
        assert!(parse_expires("amanhã").is_none());
    }
}
