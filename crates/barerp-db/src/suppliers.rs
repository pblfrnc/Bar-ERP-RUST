//! Fornecedores. Tradução de `server/src/routes/suppliers.ts`.
//!
//! O documento (CNPJ/CPF) fica só com dígitos e é único: é a mesma chave que a importação de NF-e
//! usa (`importacao::upsert_fornecedor_nfe`), então o fornecedor cadastrado à mão é reaproveitado
//! quando a primeira nota dele chega.

use serde::Serialize;
use sqlx::FromRow;

use barerp_core::ids;

use crate::{Db, DbError, DbResult, DbTime};

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Supplier {
    pub id: String,
    /// Razão social
    pub name: String,
    #[sqlx(rename = "tradeName")]
    pub trade_name: Option<String>,
    /// Só dígitos
    pub document: Option<String>,
    pub ie: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub address: Option<String>,
    #[sqlx(rename = "contactName")]
    pub contact_name: Option<String>,
    pub notes: Option<String>,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
    /// Produtos ligados ao fornecedor (`_count.products`)
    #[sqlx(rename = "productCount")]
    pub product_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SupplierProduct {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
    pub price: f64,
    pub stock: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SupplierDetail {
    #[serde(flatten)]
    pub supplier: Supplier,
    pub products: Vec<SupplierProduct>,
}

/// Campos do cadastro. Na alteração, `None` = não mexe; `Some("")` = apaga.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SupplierInput {
    pub name: Option<String>,
    pub trade_name: Option<String>,
    pub document: Option<String>,
    pub ie: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub address: Option<String>,
    pub contact_name: Option<String>,
    pub notes: Option<String>,
}

const SELECT: &str = r#"SELECT s."id", s."name", s."tradeName", s."document", s."ie", s."phone", s."email", s."city",
       s."state", s."address", s."contactName", s."notes", s."createdAt",
       (SELECT COUNT(*) FROM "Product" p WHERE p."supplierId" = s."id") AS "productCount"
FROM "Supplier" s"#;

/// CNPJ/CPF só com dígitos; vazio vira `None`.
pub fn clean_document(document: &str) -> Option<String> {
    let digits: String = document.chars().filter(|c| c.is_ascii_digit()).collect();
    (!digits.is_empty()).then_some(digits)
}

fn text(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Lista por nome; `search` procura em razão social, fantasia, documento, contato, cidade e telefone.
pub async fn list(db: &Db, search: Option<&str>) -> DbResult<Vec<Supplier>> {
    let search = search
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
    Ok(sqlx::query_as(&format!(
        r#"{SELECT}
           WHERE ? IS NULL OR s."name" LIKE ?1 OR s."tradeName" LIKE ?1 OR s."document" LIKE ?1
              OR s."contactName" LIKE ?1 OR s."city" LIKE ?1 OR s."phone" LIKE ?1
           ORDER BY s."name" ASC"#
    ))
    .bind(&search)
    .fetch_all(db.pool())
    .await?)
}

pub async fn get(db: &Db, id: &str) -> DbResult<Supplier> {
    sqlx::query_as(&format!(r#"{SELECT} WHERE s."id" = ?"#))
        .bind(id)
        .fetch_optional(db.pool())
        .await?
        .ok_or_else(|| DbError::not_found("Fornecedor não encontrado"))
}

/// Fornecedor com os produtos dele.
pub async fn detail(db: &Db, id: &str) -> DbResult<SupplierDetail> {
    let supplier = get(db, id).await?;
    let products = sqlx::query_as(
        r#"SELECT "id", "name", "code", "price", CAST("stock" AS INTEGER) AS "stock"
           FROM "Product" WHERE "supplierId" = ? ORDER BY "name""#,
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    Ok(SupplierDetail { supplier, products })
}

/// Cadastra. Recusa documento que já é de outro fornecedor.
pub async fn create(db: &Db, input: SupplierInput) -> DbResult<Supplier> {
    let name =
        text(&input.name).ok_or_else(|| DbError::rule("Razão Social / Nome é obrigatório"))?;
    let document = input.document.as_deref().and_then(clean_document);
    let id = ids::cuid();
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;
    if let Some(doc) = &document {
        let existing: Option<String> =
            sqlx::query_scalar(r#"SELECT "id" FROM "Supplier" WHERE "document" = ?"#)
                .bind(doc)
                .fetch_optional(&mut **tx)
                .await?;
        if existing.is_some() {
            return Err(DbError::rule(
                "Já existe um fornecedor cadastrado com este CNPJ/CPF",
            ));
        }
    }
    sqlx::query(
        r#"INSERT INTO "Supplier" ("id", "name", "tradeName", "document", "ie", "phone", "email", "city", "state",
                                   "address", "contactName", "notes", "createdAt", "updatedAt")
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(name)
    .bind(text(&input.trade_name))
    .bind(document)
    .bind(text(&input.ie))
    .bind(text(&input.phone))
    .bind(text(&input.email))
    .bind(text(&input.city))
    .bind(text(&input.state))
    .bind(text(&input.address))
    .bind(text(&input.contact_name))
    .bind(text(&input.notes))
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    get(db, &id).await
}

/// Altera só os campos informados.
pub async fn update(db: &Db, id: &str, input: SupplierInput) -> DbResult<Supplier> {
    let mut tx = db.begin_write().await?;
    let exists: Option<String> =
        sqlx::query_scalar(r#"SELECT "id" FROM "Supplier" WHERE "id" = ?"#)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    if exists.is_none() {
        return Err(DbError::not_found("Fornecedor não encontrado"));
    }
    if let Some(name) = &input.name {
        if name.trim().is_empty() {
            return Err(DbError::rule("Razão Social / Nome é obrigatório"));
        }
    }
    let document = input.document.as_deref().map(clean_document);
    if let Some(Some(doc)) = &document {
        let other: Option<String> =
            sqlx::query_scalar(r#"SELECT "id" FROM "Supplier" WHERE "document" = ? AND "id" <> ?"#)
                .bind(doc)
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        if other.is_some() {
            return Err(DbError::rule("Outro fornecedor já utiliza este CNPJ/CPF"));
        }
    }
    let fields: [(&str, Option<Option<String>>); 11] = [
        (
            "name",
            input.name.as_ref().map(|n| Some(n.trim().to_string())),
        ),
        (
            "tradeName",
            input.trade_name.as_ref().map(|_| text(&input.trade_name)),
        ),
        ("document", document),
        ("ie", input.ie.as_ref().map(|_| text(&input.ie))),
        ("phone", input.phone.as_ref().map(|_| text(&input.phone))),
        ("email", input.email.as_ref().map(|_| text(&input.email))),
        ("city", input.city.as_ref().map(|_| text(&input.city))),
        ("state", input.state.as_ref().map(|_| text(&input.state))),
        (
            "address",
            input.address.as_ref().map(|_| text(&input.address)),
        ),
        (
            "contactName",
            input
                .contact_name
                .as_ref()
                .map(|_| text(&input.contact_name)),
        ),
        ("notes", input.notes.as_ref().map(|_| text(&input.notes))),
    ];
    for (column, value) in fields {
        if let Some(value) = value {
            sqlx::query(&format!(
                r#"UPDATE "Supplier" SET "{column}" = ? WHERE "id" = ?"#
            ))
            .bind(value)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        }
    }
    sqlx::query(r#"UPDATE "Supplier" SET "updatedAt" = ? WHERE "id" = ?"#)
        .bind(DbTime::now())
        .bind(id)
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    get(db, id).await
}

/// Remove o fornecedor; os produtos dele ficam sem fornecedor.
pub async fn delete(db: &Db, id: &str) -> DbResult<()> {
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"UPDATE "Product" SET "supplierId" = NULL, "updatedAt" = ? WHERE "supplierId" = ?"#,
    )
    .bind(DbTime::now())
    .bind(id)
    .execute(&mut **tx)
    .await?;
    let removed = sqlx::query(r#"DELETE FROM "Supplier" WHERE "id" = ?"#)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(DbError::not_found("Fornecedor não encontrado"));
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importacao::{upsert_fornecedor_nfe, FornecedorNfe};

    fn input(name: &str, doc: Option<&str>) -> SupplierInput {
        SupplierInput {
            name: Some(name.into()),
            document: doc.map(str::to_string),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn cadastro_busca_e_documento_unico() {
        let db = Db::open_in_memory().await.unwrap();
        let a = create(
            &db,
            SupplierInput {
                city: Some(" Campinas ".into()),
                ..input(" Ambev SA ", Some("07.526.557/0001-00"))
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (a.name.as_str(), a.document.as_deref()),
            ("Ambev SA", Some("07526557000100"))
        );
        assert_eq!(a.city.as_deref(), Some("Campinas"));
        let e = create(&db, input("Outra", Some("07526557000100")))
            .await
            .unwrap_err();
        assert!(matches!(e, DbError::Rule(m) if m.contains("Já existe")));
        assert!(matches!(
            create(&db, input("  ", None)).await,
            Err(DbError::Rule(_))
        ));

        create(&db, input("Distribuidora Zé", None)).await.unwrap();
        assert_eq!(list(&db, None).await.unwrap().len(), 2);
        assert_eq!(list(&db, Some("campinas")).await.unwrap()[0].id, a.id);
        assert_eq!(list(&db, Some("7526557")).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn alteracao_parcial_e_exclusao() {
        let db = Db::open_in_memory().await.unwrap();
        let a = create(
            &db,
            SupplierInput {
                phone: Some("1133".into()),
                ..input("A", Some("111"))
            },
        )
        .await
        .unwrap();
        let b = create(&db, input("B", Some("222"))).await.unwrap();

        let e = update(
            &db,
            &b.id,
            SupplierInput {
                document: Some("111".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(e, DbError::Rule(m) if m.contains("Outro fornecedor")));
        let a2 = update(
            &db,
            &a.id,
            SupplierInput {
                email: Some("a@a.com".into()),
                phone: Some("".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (a2.email.as_deref(), a2.phone, a2.name.as_str()),
            (Some("a@a.com"), None, "A")
        );
        assert!(matches!(
            update(&db, "x", SupplierInput::default()).await,
            Err(DbError::NotFound(_))
        ));

        delete(&db, &a.id).await.unwrap();
        assert!(matches!(
            delete(&db, &a.id).await,
            Err(DbError::NotFound(_))
        ));
        assert_eq!(detail(&db, &b.id).await.unwrap().products.len(), 0);
    }

    #[tokio::test]
    async fn nota_fiscal_reaproveita_o_fornecedor_cadastrado() {
        let db = Db::open_in_memory().await.unwrap();
        let a = create(&db, input("Ambev", Some("07.526.557/0001-00")))
            .await
            .unwrap();
        let nfe = FornecedorNfe {
            documento: "07526557000100".into(),
            nome: "AMBEV S.A.".into(),
            ..Default::default()
        };
        assert_eq!(
            upsert_fornecedor_nfe(&db, &nfe).await.unwrap().as_deref(),
            Some(a.id.as_str())
        );
        assert_eq!(list(&db, None).await.unwrap().len(), 1);
    }
}
