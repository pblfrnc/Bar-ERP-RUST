//! Configurações gerais do sistema (PUT `/api/settings` do Node): nome do estabelecimento e margem de lucro padrão.
//! Licença em `barerp-license`; dashboard e auditoria em [`crate::dashboard`]; clientes e fornecedores em
//! [`crate::customers`] e [`crate::suppliers`].

use crate::{Db, DbError, DbResult, DbTime};

/// "Margem de Lucro Padrão do Sistema" (PUT `/api/settings` com `defaultProfitMargin`; 0 ou inválido vira 50).
pub async fn set_default_profit_margin(db: &Db, margin: f64) -> DbResult<()> {
    let margin = if margin.is_finite() && margin != 0.0 { margin } else { 50.0 };
    let mut tx = db.begin_write().await?;
    let changed = sqlx::query(r#"UPDATE "SystemSettings" SET "defaultProfitMargin" = ?, "updatedAt" = ? WHERE "id" = 'default'"#)
        .bind(margin)
        .bind(DbTime::now())
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Configurações do sistema não encontradas"));
    }
    tx.commit().await?;
    Ok(())
}

/// Nome do estabelecimento (PUT `/api/settings` com `restaurantName`).
pub async fn save_system_settings(db: &Db, restaurant_name: &str) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"INSERT INTO "SystemSettings" ("id", "restaurantName", "updatedAt")
           VALUES ('default', ?, ?)
           ON CONFLICT("id") DO UPDATE SET "restaurantName" = excluded."restaurantName", "updatedAt" = excluded."updatedAt""#,
    )
    .bind(restaurant_name.trim())
    .bind(DbTime::now())
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn margin_and_name() {
        let db = Db::open_in_memory().await.unwrap();
        assert!(set_default_profit_margin(&db, 40.0).await.is_err());
        save_system_settings(&db, "  JUBAR ").await.unwrap();
        set_default_profit_margin(&db, 0.0).await.unwrap();
        let (name, margin): (String, f64) =
            sqlx::query_as(r#"SELECT "restaurantName", "defaultProfitMargin" FROM "SystemSettings""#).fetch_one(db.pool()).await.unwrap();
        assert_eq!((name.as_str(), margin), ("JUBAR", 50.0));
    }
}
