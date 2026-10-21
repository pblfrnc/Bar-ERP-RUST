//! Clientes e fiado. Tradução de `server/src/routes/customers.ts`.
//!
//! Telefone, documento e observações ficam cifrados no banco (AES-256-GCM do `vault`, mesmo formato
//! do Node); dados antigos sem cifra continuam legíveis. O saldo do fiado sobe no pagamento
//! `CREDIT_TAB` da comanda (`orders`) e desce em [`pay_credit_tab`].

use serde::Serialize;
use sqlx::FromRow;

use barerp_core::{ids, vault};

use crate::{Db, DbError, DbResult, DbTime};

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Customer {
    pub id: String,
    pub name: String,
    pub phone: Option<String>,
    pub document: Option<String>,
    pub notes: Option<String>,
    /// Quanto o cliente deve no fiado
    #[sqlx(rename = "creditTabBalance")]
    pub credit_tab_balance: f64,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
}

impl Customer {
    fn opened(mut self) -> Self {
        self.phone = vault::decrypt_opt(self.phone);
        self.document = vault::decrypt_opt(self.document);
        self.notes = vault::decrypt_opt(self.notes);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewCustomer {
    pub name: String,
    pub phone: Option<String>,
    pub document: Option<String>,
    pub notes: Option<String>,
}

const SELECT: &str = r#"SELECT "id", "name", "phone", "document", "notes", "creditTabBalance", "createdAt" FROM "Customer""#;

/// Todos os clientes por nome, com os campos confidenciais abertos.
pub async fn list(db: &Db) -> DbResult<Vec<Customer>> {
    let rows: Vec<Customer> = sqlx::query_as(&format!(r#"{SELECT} ORDER BY "name" ASC"#))
        .fetch_all(db.pool())
        .await?;
    Ok(rows.into_iter().map(Customer::opened).collect())
}

pub async fn get(db: &Db, id: &str) -> DbResult<Customer> {
    let row: Option<Customer> = sqlx::query_as(&format!(r#"{SELECT} WHERE "id" = ?"#))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    row.map(Customer::opened)
        .ok_or_else(|| DbError::not_found("Cliente não encontrado"))
}

fn sealed(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(vault::encrypt_field)
}

/// Cadastra o cliente com os campos confidenciais cifrados.
pub async fn create(db: &Db, input: NewCustomer) -> DbResult<Customer> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(DbError::rule("Nome do cliente é obrigatório"));
    }
    let id = ids::cuid();
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"INSERT INTO "Customer" ("id", "name", "phone", "document", "notes", "creditTabBalance", "createdAt", "updatedAt")
           VALUES (?, ?, ?, ?, ?, 0, ?, ?)"#,
    )
    .bind(&id)
    .bind(name)
    .bind(sealed(&input.phone))
    .bind(sealed(&input.document))
    .bind(sealed(&input.notes))
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    get(db, &id).await
}

/// Recebe um pagamento do fiado. Devolve o saldo que sobra.
pub async fn pay_credit_tab(db: &Db, customer_id: &str, amount: f64) -> DbResult<f64> {
    if !amount.is_finite() || amount <= 0.0 {
        return Err(DbError::rule("Valor do pagamento deve ser maior que zero"));
    }
    let mut tx = db.begin_write().await?;
    let balance: f64 =
        sqlx::query_scalar(r#"SELECT "creditTabBalance" FROM "Customer" WHERE "id" = ?"#)
            .bind(customer_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| DbError::not_found("Cliente não encontrado"))?;
    if amount > balance + 0.009 {
        return Err(DbError::rule(format!(
            "O valor é maior que a dívida do cliente ({}).",
            barerp_core::money::brl(balance)
        )));
    }
    let left = barerp_core::pricing::round2((balance - amount).max(0.0));
    sqlx::query(r#"UPDATE "Customer" SET "creditTabBalance" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(left)
        .bind(DbTime::now())
        .bind(customer_id)
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    Ok(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn campos_cifrados_no_banco_e_abertos_na_leitura() {
        let db = Db::open_in_memory().await.unwrap();
        let c = create(
            &db,
            NewCustomer {
                name: "  Maria  ".into(),
                phone: Some(" (11) 99999-0000 ".into()),
                document: Some("123.456.789-00".into()),
                notes: Some("   ".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(c.name, "Maria");
        assert_eq!(c.phone.as_deref(), Some("(11) 99999-0000"));
        assert_eq!(c.notes, None);

        let raw: (Option<String>, Option<String>) =
            sqlx::query_as(r#"SELECT "phone", "document" FROM "Customer" WHERE "id" = ?"#)
                .bind(&c.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(
            raw.0.unwrap().starts_with("$enc$v1$"),
            "telefone cifrado no banco (formato do Node)"
        );
        assert!(!raw.1.unwrap().contains("123.456"));
        assert_eq!(
            list(&db).await.unwrap()[0].document.as_deref(),
            Some("123.456.789-00")
        );

        assert!(matches!(
            create(&db, NewCustomer::default()).await,
            Err(DbError::Rule(_))
        ));
    }

    #[tokio::test]
    async fn legado_sem_cifra_continua_legivel() {
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query(r#"INSERT INTO "Customer" ("id","name","phone","createdAt","updatedAt") VALUES ('c1','Zé','1199',CURRENT_TIMESTAMP,0)"#)
            .execute(db.pool())
            .await
            .unwrap();
        let c = get(&db, "c1").await.unwrap();
        assert_eq!(c.phone.as_deref(), Some("1199"));
    }

    #[tokio::test]
    async fn pagamento_do_fiado() {
        let db = Db::open_in_memory().await.unwrap();
        let c = create(
            &db,
            NewCustomer {
                name: "João".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        sqlx::query(r#"UPDATE "Customer" SET "creditTabBalance" = 50.3 WHERE "id" = ?"#)
            .bind(&c.id)
            .execute(db.pool())
            .await
            .unwrap();

        assert_eq!(pay_credit_tab(&db, &c.id, 20.1).await.unwrap(), 30.2);
        assert!(matches!(
            pay_credit_tab(&db, &c.id, 31.0).await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            pay_credit_tab(&db, &c.id, 0.0).await,
            Err(DbError::Rule(_))
        ));
        assert!(matches!(
            pay_credit_tab(&db, "nao-existe", 1.0).await,
            Err(DbError::NotFound(_))
        ));
        assert_eq!(pay_credit_tab(&db, &c.id, 30.2).await.unwrap(), 0.0);
    }
}
