//! Garçons (comissão e código). Tradução de `server/src/routes/waiters.ts`.

use serde::Serialize;
use sqlx::FromRow;

use barerp_core::ids;

use crate::{Db, DbError, DbResult, DbTime};

const DEFAULT_COMMISSION: f64 = 0.10;

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Waiter {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
    /// 0.10 = 10%
    #[sqlx(rename = "commissionRate")]
    pub commission_rate: f64,
    pub active: bool,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
}

/// Alteração: `None` = não mexe; `code: Some("")` apaga o código.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaiterUpdate {
    pub name: Option<String>,
    pub code: Option<String>,
    pub commission_rate: Option<f64>,
    pub active: Option<bool>,
}

/// Resultado de remover: com comandas no histórico ele só é desativado.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaiterRemoval {
    pub deactivated: bool,
    pub message: &'static str,
}

const SELECT: &str =
    r#"SELECT "id", "name", "code", "commissionRate", "active", "createdAt" FROM "Waiter""#;

/// Por nome; `all` inclui os desativados.
pub async fn list(db: &Db, all: bool) -> DbResult<Vec<Waiter>> {
    Ok(sqlx::query_as(&format!(
        r#"{SELECT} WHERE ? OR "active" = 1 ORDER BY "name" ASC"#
    ))
    .bind(all)
    .fetch_all(db.pool())
    .await?)
}

pub async fn get(db: &Db, id: &str) -> DbResult<Waiter> {
    sqlx::query_as(&format!(r#"{SELECT} WHERE "id" = ?"#))
        .bind(id)
        .fetch_optional(db.pool())
        .await?
        .ok_or_else(|| DbError::not_found("Garçom não encontrado"))
}

fn code(v: &str) -> Option<String> {
    Some(v.trim().to_string()).filter(|s| !s.is_empty())
}

fn check_rate(rate: f64) -> DbResult<f64> {
    if rate.is_finite() && (0.0..=1.0).contains(&rate) {
        Ok(rate)
    } else {
        Err(DbError::rule("Comissão inválida (use de 0 a 100%)."))
    }
}

/// Cadastra (ativo). Comissão padrão de 10%.
pub async fn create(
    db: &Db,
    name: &str,
    code_value: Option<&str>,
    commission_rate: Option<f64>,
) -> DbResult<Waiter> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DbError::rule("O nome do garçom é obrigatório"));
    }
    let rate = check_rate(commission_rate.unwrap_or(DEFAULT_COMMISSION))?;
    let id = ids::cuid();
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;
    let existing: Option<String> =
        sqlx::query_scalar(r#"SELECT "id" FROM "Waiter" WHERE "name" = ?"#)
            .bind(name)
            .fetch_optional(&mut **tx)
            .await?;
    if existing.is_some() {
        return Err(DbError::rule(
            "Já existe um garçom cadastrado com este nome",
        ));
    }
    sqlx::query(
        r#"INSERT INTO "Waiter" ("id", "name", "code", "commissionRate", "active", "createdAt", "updatedAt")
           VALUES (?, ?, ?, ?, 1, ?, ?)"#,
    )
    .bind(&id)
    .bind(name)
    .bind(code_value.and_then(code))
    .bind(rate)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    get(db, &id).await
}

pub async fn update(db: &Db, id: &str, u: WaiterUpdate) -> DbResult<Waiter> {
    let current = get(db, id).await?;
    let name = u
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&current.name)
        .to_string();
    let code_value = match &u.code {
        Some(c) => code(c),
        None => current.code.clone(),
    };
    let rate = match u.commission_rate {
        Some(r) => check_rate(r)?,
        None => current.commission_rate,
    };
    let active = u.active.unwrap_or(current.active);
    let mut tx = db.begin_write().await?;
    if name != current.name {
        let other: Option<String> =
            sqlx::query_scalar(r#"SELECT "id" FROM "Waiter" WHERE "name" = ? AND "id" <> ?"#)
                .bind(&name)
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        if other.is_some() {
            return Err(DbError::rule(
                "Já existe um garçom cadastrado com este nome",
            ));
        }
    }
    sqlx::query(
        r#"UPDATE "Waiter" SET "name" = ?, "code" = ?, "commissionRate" = ?, "active" = ?, "updatedAt" = ? WHERE "id" = ?"#,
    )
    .bind(&name)
    .bind(code_value)
    .bind(rate)
    .bind(active)
    .bind(DbTime::now())
    .bind(id)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    get(db, id).await
}

/// Exclui; se já tem comanda no histórico, só desativa (preserva o histórico financeiro).
pub async fn remove(db: &Db, id: &str) -> DbResult<WaiterRemoval> {
    let mut tx = db.begin_write().await?;
    let exists: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Waiter" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    if exists.is_none() {
        return Err(DbError::not_found("Garçom não encontrado"));
    }
    let has_orders: bool =
        sqlx::query_scalar(r#"SELECT EXISTS (SELECT 1 FROM "Order" WHERE "waiterId" = ?)"#)
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    let removal = if has_orders {
        sqlx::query(r#"UPDATE "Waiter" SET "active" = 0, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(DbTime::now())
            .bind(id)
            .execute(&mut **tx)
            .await?;
        WaiterRemoval {
            deactivated: true,
            message: "Garçom desativado com sucesso",
        }
    } else {
        sqlx::query(r#"DELETE FROM "Waiter" WHERE "id" = ?"#)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        WaiterRemoval {
            deactivated: false,
            message: "Garçom excluído com sucesso",
        }
    };
    tx.commit().await?;
    Ok(removal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cadastro_alteracao_e_nome_unico() {
        let db = Db::open_in_memory().await.unwrap();
        let ana = create(&db, " Ana ", Some(" 07 "), None).await.unwrap();
        assert_eq!(
            (
                ana.name.as_str(),
                ana.code.as_deref(),
                ana.commission_rate,
                ana.active
            ),
            ("Ana", Some("07"), 0.10, true)
        );
        assert!(matches!(
            create(&db, "Ana", None, None).await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            create(&db, "", None, None).await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            create(&db, "Bia", None, Some(1.5)).await,
            Err(DbError::Rule(_))
        ));
        let bia = create(&db, "Bia", None, Some(0.05)).await.unwrap();

        assert!(matches!(
            update(
                &db,
                &bia.id,
                WaiterUpdate {
                    name: Some("Ana".into()),
                    ..Default::default()
                }
            )
            .await,
            Err(DbError::Rule(_))
        ));
        let ana = update(
            &db,
            &ana.id,
            WaiterUpdate {
                code: Some("".into()),
                active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (ana.code, ana.active, ana.name.as_str()),
            (None, false, "Ana")
        );
        assert_eq!(list(&db, false).await.unwrap().len(), 1);
        assert_eq!(list(&db, true).await.unwrap().len(), 2);
        assert!(matches!(
            update(&db, "x", WaiterUpdate::default()).await,
            Err(DbError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn com_comanda_so_desativa() {
        let db = Db::open_in_memory().await.unwrap();
        let ana = create(&db, "Ana", None, None).await.unwrap();
        let bia = create(&db, "Bia", None, None).await.unwrap();
        sqlx::query(r#"INSERT INTO "Order" ("id","waiterId","createdAt","updatedAt") VALUES ('o1', ?, 0, 0)"#)
            .bind(&ana.id)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(remove(&db, &ana.id).await.unwrap().deactivated);
        assert!(!get(&db, &ana.id).await.unwrap().active);
        assert!(!remove(&db, &bia.id).await.unwrap().deactivated);
        assert!(matches!(get(&db, &bia.id).await, Err(DbError::NotFound(_))));
        assert!(matches!(
            remove(&db, &bia.id).await,
            Err(DbError::NotFound(_))
        ));
    }
}
