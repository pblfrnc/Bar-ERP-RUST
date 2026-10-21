//! Funcionários e login. Tradução de `server/src/routes/staff.ts` (`/public-list` e `/login`).

use chrono::{Duration, Utc};
use serde::Serialize;
use sqlx::FromRow;

use barerp_core::vault;

use crate::migrate::ALL_PERMISSIONS;
use barerp_core::ids;

use crate::{Db, DbError, DbResult, DbTime};

const MAX_ATTEMPTS: i64 = 5;
const LOCKOUT_SECONDS: i64 = 15;
/// Senha do Administrador criado na instalação. Diferente do Node, não existe senha mestra (9999):
/// quem esquecer a senha pede a um administrador para trocar.
pub const DEFAULT_PASSWORD: &str = "1234";
const MIN_PASSWORD_LEN: usize = 4;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct StaffSummary {
    pub id: String,
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoggedUser {
    pub id: String,
    pub name: String,
    pub role: String,
    pub permissions: Vec<String>,
    /// Entrou com a senha padrão 1234: a tela sugere trocar
    pub default_password: bool,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LoginError {
    #[error("Usuário e senha são obrigatórios")]
    Missing,
    #[error("Usuário não encontrado")]
    NotFound,
    #[error("Usuário desativado pelo administrador")]
    Inactive,
    #[error("Muitas tentativas incorretas. Aguarde {0} segundos para tentar novamente.")]
    LockedOut(i64),
    #[error("5 tentativas incorretas. Acesso liberado automaticamente em 15 segundos.")]
    JustLocked,
    #[error("Senha incorreta. Tentativa {0} de 5.")]
    WrongPassword(i64),
    #[error("Erro interno ao processar login")]
    Internal,
}

#[derive(FromRow)]
struct StaffRow {
    id: String,
    name: String,
    role: String,
    password: String,
    permissions: String,
    active: bool,
    #[sqlx(rename = "failedAttempts")]
    failed_attempts: i64,
    #[sqlx(rename = "lockoutUntil")]
    lockout_until: Option<DbTime>,
}

/// Funcionários ativos para a tela de login, em ordem alfabética.
pub async fn public_list(db: &Db) -> DbResult<Vec<StaffSummary>> {
    Ok(sqlx::query_as(r#"SELECT "id", "name", "role" FROM "Staff" WHERE "active" = 1 ORDER BY "name" ASC"#)
        .fetch_all(db.pool())
        .await?)
}

pub async fn login(db: &Db, user_id: &str, password: &str) -> Result<LoggedUser, LoginError> {
    if user_id.is_empty() || password.is_empty() {
        return Err(LoginError::Missing);
    }
    let user: StaffRow = sqlx::query_as(
        r#"SELECT "id", "name", "role", "password", "permissions", "active", "failedAttempts", "lockoutUntil"
           FROM "Staff" WHERE "id" = ?"#,
    )
    .bind(user_id)
    .fetch_optional(db.pool())
    .await
    .map_err(|_| LoginError::Internal)?
    .ok_or(LoginError::NotFound)?;

    if !user.active {
        return Err(LoginError::Inactive);
    }

    let now = Utc::now();
    if let Some(DbTime(until)) = user.lockout_until {
        if until > now {
            let remaining_ms = (until - now).num_milliseconds();
            return Err(LoginError::LockedOut((remaining_ms + 999) / 1000));
        }
    }

    let valid = vault::verify_password(password.trim(), &user.password);
    let update = r#"UPDATE "Staff" SET "failedAttempts" = ?, "lockoutUntil" = ?, "updatedAt" = ? WHERE "id" = ?"#;

    if !valid {
        let next = user.failed_attempts + 1;
        let will_lock = next >= MAX_ATTEMPTS;
        let lockout = will_lock.then(|| DbTime(now + Duration::seconds(LOCKOUT_SECONDS)));
        sqlx::query(update)
            .bind(if will_lock { 0 } else { next })
            .bind(lockout)
            .bind(DbTime(now))
            .bind(&user.id)
            .execute(db.pool())
            .await
            .map_err(|_| LoginError::Internal)?;
        return Err(if will_lock { LoginError::JustLocked } else { LoginError::WrongPassword(next) });
    }

    sqlx::query(update)
        .bind(0)
        .bind(None::<DbTime>)
        .bind(DbTime(now))
        .bind(&user.id)
        .execute(db.pool())
        .await
        .map_err(|_| LoginError::Internal)?;

    let mut permissions: Vec<String> =
        serde_json::from_str(&user.permissions).unwrap_or_else(|_| vec!["tables".into(), "cash".into()]);
    if user.role == "ADMIN" && permissions.is_empty() {
        permissions = ALL_PERMISSIONS.iter().map(|p| p.to_string()).collect();
    }

    Ok(LoggedUser {
        id: user.id,
        name: user.name,
        role: user.role,
        permissions,
        default_password: password.trim() == DEFAULT_PASSWORD,
    })
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ChangePasswordError {
    #[error("A senha atual não confere.")]
    WrongCurrent,
    #[error("A nova senha precisa ter pelo menos 4 caracteres.")]
    TooShort,
    #[error("A confirmação não é igual à nova senha.")]
    Mismatch,
    #[error("A nova senha precisa ser diferente da atual.")]
    Unchanged,
    #[error("Usuário não encontrado")]
    NotFound,
    #[error("Erro ao salvar a nova senha")]
    Internal,
}

/// Troca da própria senha pelo funcionário logado (confere a senha atual).
pub async fn change_password(
    db: &Db,
    user_id: &str,
    current: &str,
    new: &str,
    confirm: &str,
) -> Result<(), ChangePasswordError> {
    let (current, new, confirm) = (current.trim(), new.trim(), confirm.trim());
    let stored: String = sqlx::query_scalar(r#"SELECT "password" FROM "Staff" WHERE "id" = ?"#)
        .bind(user_id)
        .fetch_optional(db.pool())
        .await
        .map_err(|_| ChangePasswordError::Internal)?
        .ok_or(ChangePasswordError::NotFound)?;

    if !vault::verify_password(current, &stored) {
        return Err(ChangePasswordError::WrongCurrent);
    }
    if new.chars().count() < MIN_PASSWORD_LEN {
        return Err(ChangePasswordError::TooShort);
    }
    if new != confirm {
        return Err(ChangePasswordError::Mismatch);
    }
    if new == current {
        return Err(ChangePasswordError::Unchanged);
    }

    sqlx::query(r#"UPDATE "Staff" SET "password" = ?, "failedAttempts" = 0, "lockoutUntil" = NULL, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(vault::hash_password(new))
        .bind(DbTime::now())
        .bind(user_id)
        .execute(db.pool())
        .await
        .map_err(|_| ChangePasswordError::Internal)?;
    Ok(())
}

// ---------- Cadastro de funcionários (ManageStaffModal / routes/staff.ts) ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaffMember {
    pub id: String,
    pub name: String,
    pub role: String,
    pub permissions: Vec<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StaffInput {
    pub name: String,
    pub role: String,
    /// Vazio na edição = mantém a senha atual
    pub password: String,
    pub permissions: Vec<String>,
    pub active: bool,
}

fn parse_permissions(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_else(|_| vec!["tables".into(), "cash".into()])
}

/// Todos os funcionários: ativos primeiro, depois por nome.
pub async fn list_all(db: &Db) -> DbResult<Vec<StaffMember>> {
    let rows: Vec<(String, String, String, String, bool)> = sqlx::query_as(
        r#"SELECT "id", "name", "role", "permissions", "active" FROM "Staff" ORDER BY "active" DESC, "name" ASC"#,
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, role, permissions, active)| StaffMember { id, name, role, permissions: parse_permissions(&permissions), active })
        .collect())
}

async fn active_admins(conn: &mut sqlx::SqliteConnection) -> DbResult<i64> {
    Ok(sqlx::query_scalar(r#"SELECT COUNT(*) FROM "Staff" WHERE "role" = 'ADMIN' AND "active" = 1"#).fetch_one(conn).await?)
}

pub async fn create_staff(db: &Db, input: StaffInput) -> DbResult<StaffMember> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(DbError::rule("O nome do funcionário é obrigatório"));
    }
    let password = input.password.trim();
    if password.is_empty() {
        return Err(DbError::rule("A senha ou PIN de acesso é obrigatória"));
    }
    let role = if input.role.trim().is_empty() { "OPERADOR".to_string() } else { input.role.trim().to_string() };
    let permissions = if input.permissions.is_empty() { vec!["tables".to_string(), "cash".to_string()] } else { input.permissions };
    let id = ids::cuid();
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"INSERT INTO "Staff" ("id", "name", "role", "password", "permissions", "active", "failedAttempts", "createdAt", "updatedAt")
           VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)"#,
    )
    .bind(&id)
    .bind(&name)
    .bind(&role)
    .bind(vault::hash_password(password))
    .bind(serde_json::to_string(&permissions).expect("lista de textos"))
    .bind(input.active)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(StaffMember { id, name, role, permissions, active: input.active })
}

pub async fn update_staff(db: &Db, id: &str, input: StaffInput) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    let current: Option<(String, bool)> = sqlx::query_as(r#"SELECT "role", "active" FROM "Staff" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    let (role, active) = current.ok_or_else(|| DbError::not_found("Funcionário não encontrado"))?;
    // Prevenir desativar (ou rebaixar) o único administrador
    let losing_admin = role == "ADMIN" && active && (!input.active || input.role.trim() != "ADMIN");
    if losing_admin && active_admins(&mut tx).await? <= 1 {
        return Err(DbError::rule("Não é permitido desativar o único Administrador do sistema"));
    }
    let name = input.name.trim();
    if name.is_empty() {
        return Err(DbError::rule("O nome do funcionário é obrigatório"));
    }
    sqlx::query(r#"UPDATE "Staff" SET "name" = ?, "role" = ?, "permissions" = ?, "active" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(name)
        .bind(input.role.trim())
        .bind(serde_json::to_string(&input.permissions).expect("lista de textos"))
        .bind(input.active)
        .bind(DbTime::now())
        .bind(id)
        .execute(&mut **tx)
        .await?;
    if !input.password.trim().is_empty() {
        sqlx::query(r#"UPDATE "Staff" SET "password" = ?, "failedAttempts" = 0, "lockoutUntil" = NULL WHERE "id" = ?"#)
            .bind(vault::hash_password(input.password.trim()))
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn delete_staff(db: &Db, id: &str) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    let role: String = sqlx::query_scalar(r#"SELECT "role" FROM "Staff" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::not_found("Funcionário não encontrado"))?;
    if role == "ADMIN" && active_admins(&mut tx).await? <= 1 {
        return Err(DbError::rule("Não é permitido excluir o único Administrador do sistema"));
    }
    sqlx::query(r#"DELETE FROM "Staff" WHERE "id" = ?"#).bind(id).execute(&mut **tx).await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admin_is_created_and_lockout_works() {
        let db = Db::open_in_memory().await.unwrap();
        let staff = public_list(&db).await.unwrap();
        assert_eq!(staff.len(), 1);
        let admin = &staff[0];
        assert_eq!((admin.name.as_str(), admin.role.as_str()), ("Administrador", "ADMIN"));

        let user = login(&db, &admin.id, "1234").await.unwrap();
        assert_eq!(user.permissions.len(), 9);
        assert!(user.default_password);
        assert_eq!(login(&db, &admin.id, "9999").await.unwrap_err(), LoginError::WrongPassword(1));
        login(&db, &admin.id, "1234").await.unwrap();

        for attempt in 1..=4 {
            assert_eq!(login(&db, &admin.id, "0000").await.unwrap_err(), LoginError::WrongPassword(attempt));
        }
        assert_eq!(login(&db, &admin.id, "0000").await.unwrap_err(), LoginError::JustLocked);
        assert!(matches!(login(&db, &admin.id, "1234").await.unwrap_err(), LoginError::LockedOut(s) if s <= 15));
        assert_eq!(login(&db, "nao-existe", "1234").await.unwrap_err(), LoginError::NotFound);
    }

    #[tokio::test]
    async fn change_own_password() {
        let db = Db::open_in_memory().await.unwrap();
        let id = public_list(&db).await.unwrap()[0].id.clone();
        use ChangePasswordError::*;
        assert_eq!(change_password(&db, &id, "0000", "abcd", "abcd").await, Err(WrongCurrent));
        assert_eq!(change_password(&db, &id, "1234", "12", "12").await, Err(TooShort));
        assert_eq!(change_password(&db, &id, "1234", "abcd", "abce").await, Err(Mismatch));
        assert_eq!(change_password(&db, &id, "1234", "1234", "1234").await, Err(Unchanged));
        change_password(&db, &id, "1234", " Bar2026 ", "Bar2026").await.unwrap();
        assert!(login(&db, &id, "1234").await.is_err());
        let user = login(&db, &id, "Bar2026").await.unwrap();
        assert!(!user.default_password);
    }

    #[tokio::test]
    async fn staff_crud_protects_last_admin() {
        let db = Db::open_in_memory().await.unwrap();
        let admin = list_all(&db).await.unwrap()[0].clone();
        assert!(delete_staff(&db, &admin.id).await.is_err());
        let off = StaffInput { name: "Administrador".into(), role: "ADMIN".into(), active: false, permissions: admin.permissions.clone(), ..Default::default() };
        assert!(update_staff(&db, &admin.id, off).await.is_err());

        assert!(create_staff(&db, StaffInput { name: "Ana".into(), ..Default::default() }).await.is_err(), "senha obrigatória");
        let ana = create_staff(
            &db,
            StaffInput { name: " Ana Caixa ".into(), role: "CAIXA".into(), password: "4321".into(), permissions: vec!["tables".into(), "cash".into(), "customers".into()], active: true },
        )
        .await
        .unwrap();
        assert_eq!((ana.name.as_str(), ana.permissions.len()), ("Ana Caixa", 3));
        assert_eq!(login(&db, &ana.id, "4321").await.unwrap().role, "CAIXA");

        update_staff(&db, &ana.id, StaffInput { name: "Ana".into(), role: "GERENTE".into(), password: String::new(), permissions: vec!["tables".into()], active: true })
            .await
            .unwrap();
        assert!(login(&db, &ana.id, "4321").await.is_ok(), "senha vazia na edição mantém a atual");
        let list = list_all(&db).await.unwrap();
        assert!(list.iter().any(|s| s.name == "Ana" && s.role == "GERENTE" && s.permissions == vec!["tables".to_string()]));
        delete_staff(&db, &ana.id).await.unwrap();
        assert_eq!(list_all(&db).await.unwrap().len(), 1);
    }
}
