//! Auditoria cega (`AuditLog`): cancelamentos, descontos, perdas, sangrias.

use sqlx::SqliteConnection;

use barerp_core::ids;

use crate::{DbResult, DbTime};

/// Registra uma ação sensível. `user` é quem fez (o Node não gravava; aqui vai em `userId`).
pub async fn record(conn: &mut SqliteConnection, action: &str, description: &str, user: Option<&str>) -> DbResult<()> {
    sqlx::query(r#"INSERT INTO "AuditLog" ("id", "action", "description", "userId", "createdAt") VALUES (?, ?, ?, ?, ?)"#)
        .bind(ids::cuid())
        .bind(action)
        .bind(description)
        .bind(user)
        .bind(DbTime::now())
        .execute(conn)
        .await?;
    Ok(())
}
