//! Adicionais e perguntas do pedido (ponto da carne, gelo e limão, adicionais pagos): cadastro dos grupos
//! e o vínculo com os produtos. Tradução das rotas `/products/modifier-groups` e `/products/:id/modifier-groups`
//! de `server/src/routes/products.ts` (a leitura para lançar pedido fica em `catalog`).

use serde::Serialize;
use sqlx::FromRow;

use barerp_core::ids;

use crate::events::Event;
use crate::{Db, DbError, DbResult, DbTime};

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OptionRow {
    pub id: String,
    #[sqlx(rename = "groupId")]
    pub group_id: String,
    pub name: String,
    pub price: f64,
    #[sqlx(rename = "isActive")]
    pub is_active: bool,
    #[sqlx(rename = "sortOrder")]
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRow {
    pub id: String,
    pub name: String,
    pub min_select: i64,
    /// 0 = sem limite
    pub max_select: i64,
    pub is_active: bool,
    pub options: Vec<OptionRow>,
    /// Em quantos produtos o grupo está
    pub product_count: i64,
}

impl GroupRow {
    /// "Obrigatório · escolha 1", "Opcional · sem limite", "Opcional · até 3" (`describeModifierRule`).
    pub fn rule_label(&self) -> String {
        describe_rule(self.min_select, self.max_select)
    }

    /// "Mal passada (sem custo) · Bacon (+R$ 4,00) [inativa]" ou "Sem opções".
    pub fn options_label(&self) -> String {
        if self.options.is_empty() {
            return "Sem opções".into();
        }
        self.options
            .iter()
            .map(|o| {
                let price = if o.price > 0.0 { format!("+R$ {:.2}", o.price).replace('.', ",") } else { "sem custo".into() };
                format!("{} ({price}){}", o.name, if o.is_active { "" } else { " [inativa]" })
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

pub fn describe_rule(min_select: i64, max_select: i64) -> String {
    let kind = if min_select >= 1 { "Obrigatório" } else { "Opcional" };
    match max_select {
        1 => format!("{kind} · escolha 1"),
        0 => format!("{kind} · sem limite"),
        n => format!("{kind} · até {n}"),
    }
}

#[derive(FromRow)]
struct GroupHead {
    id: String,
    name: String,
    #[sqlx(rename = "minSelect")]
    min_select: i64,
    #[sqlx(rename = "maxSelect")]
    max_select: i64,
    #[sqlx(rename = "isActive")]
    is_active: bool,
    #[sqlx(rename = "productCount")]
    product_count: i64,
}

/// Todos os grupos (inclusive inativos) com todas as opções, na ordem do cadastro.
pub async fn list(db: &Db) -> DbResult<Vec<GroupRow>> {
    let heads: Vec<GroupHead> = sqlx::query_as(
        r#"SELECT g."id", g."name", g."minSelect", g."maxSelect", g."isActive",
                  (SELECT COUNT(*) FROM "ProductModifierGroup" l WHERE l."groupId" = g."id") AS "productCount"
           FROM "ModifierGroup" g ORDER BY g."sortOrder", g."name""#,
    )
    .fetch_all(db.pool())
    .await?;
    let options: Vec<OptionRow> = sqlx::query_as(
        r#"SELECT "id", "groupId", "name", "price", "isActive", "sortOrder" FROM "ModifierOption" ORDER BY "sortOrder", "name""#,
    )
    .fetch_all(db.pool())
    .await?;
    Ok(heads
        .into_iter()
        .map(|h| GroupRow {
            options: options.iter().filter(|o| o.group_id == h.id).cloned().collect(),
            id: h.id,
            name: h.name,
            min_select: h.min_select,
            max_select: h.max_select,
            is_active: h.is_active,
            product_count: h.product_count,
        })
        .collect())
}

/// Opção do formulário: `id` de opção existente ou `None` para nova.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OptionInput {
    pub id: Option<String>,
    pub name: String,
    pub price: f64,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroupInput {
    pub name: String,
    /// "Pergunta obrigatória" = 1
    pub min_select: i64,
    pub max_select: i64,
    pub is_active: bool,
    /// Na ordem da tela (vira o `sortOrder`); opções sem nome são ignoradas
    pub options: Vec<OptionInput>,
}

fn event() -> Vec<Event> {
    vec![Event::CatalogUpdated { scope: "modifiers" }]
}

/// Mesmas regras do `parseModifierGroupBody` do Node.
fn validate(input: &GroupInput) -> DbResult<Vec<OptionInput>> {
    let min = input.min_select.max(0);
    let max = input.max_select.max(0);
    if input.name.trim().is_empty() {
        return Err(DbError::rule("Informe o nome do grupo (ex: Ponto da carne)."));
    }
    if max > 0 && min > max {
        return Err(DbError::rule("O mínimo de escolhas não pode ser maior que o máximo."));
    }
    let options: Vec<OptionInput> = input
        .options
        .iter()
        .filter(|o| !o.name.trim().is_empty())
        .map(|o| OptionInput {
            id: o.id.clone().filter(|i| !i.is_empty()),
            name: o.name.trim().to_string(),
            price: if o.price.is_finite() { o.price.max(0.0) } else { 0.0 },
            is_active: o.is_active,
        })
        .collect();
    if options.is_empty() {
        return Err(DbError::rule("Cadastre pelo menos uma opção."));
    }
    if min > options.iter().filter(|o| o.is_active).count() as i64 {
        return Err(DbError::rule("O grupo exige mais escolhas do que opções ativas."));
    }
    Ok(options)
}

/// Cria (`id = None`) ou altera o grupo. Opções tiradas são apagadas (os itens já lançados guardam o nome e o preço).
pub async fn save(db: &Db, id: Option<&str>, input: GroupInput) -> DbResult<(String, Vec<Event>)> {
    let options = validate(&input)?;
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;
    let group_id = match id {
        Some(id) => {
            let changed = sqlx::query(
                r#"UPDATE "ModifierGroup" SET "name" = ?, "minSelect" = ?, "maxSelect" = ?, "isActive" = ?, "updatedAt" = ? WHERE "id" = ?"#,
            )
            .bind(input.name.trim())
            .bind(input.min_select.max(0))
            .bind(input.max_select.max(0))
            .bind(input.is_active)
            .bind(now)
            .bind(id)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            if changed == 0 {
                return Err(DbError::not_found("Grupo não encontrado"));
            }
            id.to_string()
        }
        None => {
            let id = ids::cuid();
            sqlx::query(
                r#"INSERT INTO "ModifierGroup" ("id", "name", "minSelect", "maxSelect", "isActive", "createdAt", "updatedAt")
                   VALUES (?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(input.name.trim())
            .bind(input.min_select.max(0))
            .bind(input.max_select.max(0))
            .bind(input.is_active)
            .bind(now)
            .bind(now)
            .execute(&mut **tx)
            .await?;
            id
        }
    };
    let existing: Vec<String> = sqlx::query_scalar(r#"SELECT "id" FROM "ModifierOption" WHERE "groupId" = ?"#)
        .bind(&group_id)
        .fetch_all(&mut **tx)
        .await?;
    let kept: Vec<&str> = options.iter().filter_map(|o| o.id.as_deref()).filter(|i| existing.iter().any(|e| e == i)).collect();
    for old in existing.iter().filter(|e| !kept.contains(&e.as_str())) {
        sqlx::query(r#"DELETE FROM "ModifierOption" WHERE "id" = ?"#).bind(old).execute(&mut **tx).await?;
    }
    for (i, o) in options.iter().enumerate() {
        match o.id.as_deref().filter(|i| kept.contains(i)) {
            Some(oid) => {
                sqlx::query(r#"UPDATE "ModifierOption" SET "name" = ?, "price" = ?, "isActive" = ?, "sortOrder" = ? WHERE "id" = ?"#)
                    .bind(&o.name)
                    .bind(o.price)
                    .bind(o.is_active)
                    .bind(i as i64)
                    .bind(oid)
                    .execute(&mut **tx)
                    .await?;
            }
            None => {
                sqlx::query(
                    r#"INSERT INTO "ModifierOption" ("id", "groupId", "name", "price", "isActive", "sortOrder") VALUES (?, ?, ?, ?, ?, ?)"#,
                )
                .bind(ids::cuid())
                .bind(&group_id)
                .bind(&o.name)
                .bind(o.price)
                .bind(o.is_active)
                .bind(i as i64)
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok((group_id, event()))
}

/// Exclui o grupo (sai de todos os produtos).
pub async fn delete(db: &Db, id: &str) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    sqlx::query(r#"DELETE FROM "ProductModifierGroup" WHERE "groupId" = ?"#).bind(id).execute(&mut **tx).await?;
    sqlx::query(r#"DELETE FROM "ModifierOption" WHERE "groupId" = ?"#).bind(id).execute(&mut **tx).await?;
    let removed = sqlx::query(r#"DELETE FROM "ModifierGroup" WHERE "id" = ?"#).bind(id).execute(&mut **tx).await?;
    if removed.rows_affected() == 0 {
        return Err(DbError::not_found("Grupo não encontrado"));
    }
    tx.commit().await?;
    Ok(event())
}

/// Grupos que aparecem ao lançar o produto, na ordem dada (repetidos são ignorados).
pub async fn set_product_groups(db: &Db, product_id: &str, group_ids: &[String]) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let exists: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Product" WHERE "id" = ?"#)
        .bind(product_id)
        .fetch_optional(&mut **tx)
        .await?;
    if exists.is_none() {
        return Err(DbError::not_found("Produto não encontrado"));
    }
    sqlx::query(r#"DELETE FROM "ProductModifierGroup" WHERE "productId" = ?"#).bind(product_id).execute(&mut **tx).await?;
    let mut seen: Vec<&str> = Vec::new();
    for g in group_ids.iter().map(String::as_str).filter(|g| !g.is_empty()) {
        if seen.contains(&g) {
            continue;
        }
        sqlx::query(r#"INSERT INTO "ProductModifierGroup" ("productId", "groupId", "sortOrder") VALUES (?, ?, ?)"#)
            .bind(product_id)
            .bind(g)
            .bind(seen.len() as i64)
            .execute(&mut **tx)
            .await?;
        seen.push(g);
    }
    tx.commit().await?;
    Ok(vec![Event::CatalogUpdated { scope: "products" }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(id: Option<&str>, name: &str, price: f64, active: bool) -> OptionInput {
        OptionInput { id: id.map(str::to_string), name: name.into(), price, is_active: active }
    }

    #[tokio::test]
    async fn cria_edita_e_valida() {
        let db = Db::open_in_memory().await.unwrap();
        let ponto = GroupInput {
            name: " Ponto da carne ".into(),
            min_select: 1,
            max_select: 1,
            is_active: true,
            options: vec![opt(None, "Mal passada", 0.0, true), opt(None, "  ", 0.0, true), opt(None, "Bem passada", -3.0, true)],
        };
        let (id, ev) = save(&db, None, ponto).await.unwrap();
        assert_eq!(ev, vec![Event::CatalogUpdated { scope: "modifiers" }]);
        let g = &list(&db).await.unwrap()[0];
        assert_eq!((g.name.as_str(), g.rule_label().as_str()), ("Ponto da carne", "Obrigatório · escolha 1"));
        assert_eq!(g.options_label(), "Mal passada (sem custo) · Bem passada (sem custo)");

        // Edita: tira "Mal passada", renomeia a outra, acrescenta uma paga
        let keep = g.options[1].id.clone();
        let edit = GroupInput {
            name: "Adicionais".into(),
            min_select: 0,
            max_select: 0,
            is_active: true,
            options: vec![opt(Some(&keep), "Ao ponto", 0.0, true), opt(None, "Bacon", 4.0, false)],
        };
        save(&db, Some(&id), edit).await.unwrap();
        let g = &list(&db).await.unwrap()[0];
        assert_eq!(g.options.len(), 2);
        assert_eq!(g.options[0].id, keep);
        assert_eq!(g.options_label(), "Ao ponto (sem custo) · Bacon (+R$ 4,00) [inativa]");
        assert_eq!(g.rule_label(), "Opcional · sem limite");

        let bad = |min, max, options| GroupInput { name: "X".into(), min_select: min, max_select: max, is_active: true, options };
        assert!(matches!(save(&db, None, bad(0, 0, vec![])).await, Err(DbError::Rule(_))));
        assert!(matches!(save(&db, None, bad(3, 2, vec![opt(None, "a", 0.0, true)])).await, Err(DbError::Rule(_))));
        let e = save(&db, None, bad(1, 1, vec![opt(None, "a", 0.0, false)])).await.unwrap_err();
        assert!(matches!(e, DbError::Rule(m) if m.contains("mais escolhas do que opções ativas")));
        assert!(matches!(save(&db, Some("x"), bad(0, 1, vec![opt(None, "a", 0.0, true)])).await, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn vinculo_com_produto_e_exclusao() {
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query(r#"INSERT INTO "Category" ("id","name") VALUES ('c','Cozinha')"#).execute(db.pool()).await.unwrap();
        sqlx::query(r#"INSERT INTO "Product" ("id","name","price","categoryId","updatedAt") VALUES ('p','X',30,'c',0)"#)
            .execute(db.pool())
            .await
            .unwrap();
        let mk = |n: &str| GroupInput { name: n.into(), max_select: 1, is_active: true, options: vec![opt(None, "a", 0.0, true)], ..Default::default() };
        let (a, _) = save(&db, None, mk("A")).await.unwrap();
        let (b, _) = save(&db, None, mk("B")).await.unwrap();
        set_product_groups(&db, "p", &[b.clone(), a.clone(), b.clone()]).await.unwrap();
        let order: Vec<String> = sqlx::query_scalar(r#"SELECT "groupId" FROM "ProductModifierGroup" WHERE "productId" = 'p' ORDER BY "sortOrder""#)
            .fetch_all(db.pool())
            .await
            .unwrap();
        assert_eq!(order, vec![b.clone(), a.clone()]);
        assert_eq!(list(&db).await.unwrap().iter().map(|g| g.product_count).collect::<Vec<_>>(), vec![1, 1]);
        assert!(matches!(set_product_groups(&db, "x", &[]).await, Err(DbError::NotFound(_))));

        delete(&db, &a).await.unwrap();
        assert_eq!(list(&db).await.unwrap().len(), 1);
        assert!(matches!(delete(&db, &a).await, Err(DbError::NotFound(_))));
    }
}
