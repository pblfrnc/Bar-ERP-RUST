//! Cadastro de mesas e locais/setores (Nova Mesa, Editar Locais & Mesas). Tradução de
//! `server/src/routes/tables.ts` (POST/PUT/DELETE, batch/section, sections/rename).
//! Diferenças de propósito: número de mesa precisa ser maior que zero; excluir mesa recusa também quando há
//! comanda aberta nela (o Node olhava só o status).

use barerp_core::ids;

use crate::events::Event;
use crate::orders::open_order_of_table;
use crate::{Db, DbError, DbResult, DbTime};

pub const DEFAULT_SECTION: &str = "Salão Principal";

#[derive(Debug, Clone, Default)]
pub struct TableInput {
    pub number: i64,
    pub name: Option<String>,
    pub capacity: i64,
    pub section: Option<String>,
}

fn clean(text: Option<&str>) -> Option<String> {
    text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
}

/// Nova mesa (livre). Nome padrão "Mesa N", capacidade padrão 4, setor padrão "Salão Principal".
pub async fn create_table(db: &Db, input: TableInput) -> DbResult<(String, Vec<Event>)> {
    if input.number <= 0 {
        return Err(DbError::rule("Informe um número de mesa válido"));
    }
    let mut tx = db.begin_write().await?;
    let exists: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Table" WHERE "number" = ?"#)
        .bind(input.number)
        .fetch_optional(&mut **tx)
        .await?;
    if exists.is_some() {
        return Err(DbError::rule(format!("Mesa {} já existe", input.number)));
    }
    let id = ids::cuid();
    let now = DbTime::now();
    sqlx::query(
        r#"INSERT INTO "Table" ("id", "number", "name", "capacity", "status", "section", "customerCount", "createdAt", "updatedAt")
           VALUES (?, ?, ?, ?, 'AVAILABLE', ?, 1, ?, ?)"#,
    )
    .bind(&id)
    .bind(input.number)
    .bind(clean(input.name.as_deref()).unwrap_or_else(|| format!("Mesa {}", input.number)))
    .bind(if input.capacity > 0 { input.capacity } else { 4 })
    .bind(clean(input.section.as_deref()).unwrap_or_else(|| DEFAULT_SECTION.to_string()))
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok((id.clone(), vec![Event::table(&id, "created")]))
}

/// Altera número, nome, capacidade e setor da mesa.
pub async fn update_table(db: &Db, id: &str, input: TableInput) -> DbResult<Vec<Event>> {
    if input.number <= 0 {
        return Err(DbError::rule("Informe um número de mesa válido"));
    }
    let mut tx = db.begin_write().await?;
    let found: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Table" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    if found.is_none() {
        return Err(DbError::not_found("Mesa não encontrada"));
    }
    let duplicate: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Table" WHERE "number" = ? AND "id" <> ?"#)
        .bind(input.number)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    if duplicate.is_some() {
        return Err(DbError::rule(format!("Já existe uma mesa cadastrada com o número {}", input.number)));
    }
    sqlx::query(
        r#"UPDATE "Table" SET "number" = ?, "name" = ?, "capacity" = ?, "section" = ?, "updatedAt" = ? WHERE "id" = ?"#,
    )
    .bind(input.number)
    .bind(clean(input.name.as_deref()))
    .bind(input.capacity.max(1))
    .bind(clean(input.section.as_deref()).unwrap_or_else(|| DEFAULT_SECTION.to_string()))
    .bind(DbTime::now())
    .bind(id)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(vec![Event::table(id, "edited")])
}

/// Exclui mesa livre. Devolve a mensagem de sucesso.
pub async fn delete_table(db: &Db, id: &str) -> DbResult<(String, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let row: Option<(i64, String)> = sqlx::query_as(r#"SELECT "number", "status" FROM "Table" WHERE "id" = ?"#)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    let (number, status) = row.ok_or_else(|| DbError::not_found("Mesa não encontrada"))?;
    if status != "AVAILABLE" || open_order_of_table(&mut tx, id).await?.is_some() {
        return Err(DbError::rule("Apenas mesas livres podem ser excluídas"));
    }
    // Comandas antigas continuam no histórico, só sem a mesa
    sqlx::query(r#"UPDATE "Order" SET "tableId" = NULL WHERE "tableId" = ?"#).bind(id).execute(&mut **tx).await?;
    sqlx::query(r#"UPDATE "Table" SET "mergedIntoTableId" = NULL WHERE "mergedIntoTableId" = ?"#)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(r#"DELETE FROM "Table" WHERE "id" = ?"#).bind(id).execute(&mut **tx).await?;
    tx.commit().await?;
    Ok((format!("Mesa {number} excluída com sucesso"), vec![Event::table(id, "deleted")]))
}

/// Move várias mesas para outro setor. Devolve quantas.
pub async fn move_to_section(db: &Db, table_ids: &[String], section: &str) -> DbResult<(u64, Vec<Event>)> {
    let section = section.trim();
    if table_ids.is_empty() || section.is_empty() {
        return Err(DbError::rule("Escolha as mesas e o local de destino"));
    }
    let mut tx = db.begin_write().await?;
    let now = DbTime::now();
    let mut count = 0;
    let mut events = Vec::new();
    for id in table_ids {
        count += sqlx::query(r#"UPDATE "Table" SET "section" = ?, "updatedAt" = ? WHERE "id" = ?"#)
            .bind(section)
            .bind(now)
            .bind(id)
            .execute(&mut **tx)
            .await?
            .rows_affected();
        events.push(Event::table(id, "batch_section_updated"));
    }
    tx.commit().await?;
    Ok((count, events))
}

/// Renomeia um setor em todas as mesas. Devolve quantas mudaram.
pub async fn rename_section(db: &Db, old: &str, new: &str) -> DbResult<(u64, Vec<Event>)> {
    let (old, new) = (old.trim(), new.trim());
    if old.is_empty() || new.is_empty() {
        return Err(DbError::rule("Local antigo e novo são obrigatórios"));
    }
    let mut tx = db.begin_write().await?;
    let ids: Vec<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Table" WHERE "section" = ?"#)
        .bind(old)
        .fetch_all(&mut **tx)
        .await?;
    sqlx::query(r#"UPDATE "Table" SET "section" = ?, "updatedAt" = ? WHERE "section" = ?"#)
        .bind(new)
        .bind(DbTime::now())
        .bind(old)
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    let events = ids.iter().map(|id| Event::table(id, "section_renamed")).collect();
    Ok((ids.len() as u64, events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{open_table, OpenTable};
    use crate::tables::list_cards;

    #[tokio::test]
    async fn crud_and_sections() {
        let db = Db::open_in_memory().await.unwrap();
        let (t1, _) = create_table(&db, TableInput { number: 7, capacity: 0, ..Default::default() }).await.unwrap();
        let (t2, _) = create_table(
            &db,
            TableInput { number: 8, name: Some(" VIP 02 ".into()), capacity: 6, section: Some("Deck Externo".into()) },
        )
        .await
        .unwrap();
        assert!(create_table(&db, TableInput { number: 7, ..Default::default() }).await.is_err());

        let cards = list_cards(&db).await.unwrap();
        assert_eq!(
            cards.iter().map(|c| (c.number, c.name.clone().unwrap(), c.capacity, c.section.clone())).collect::<Vec<_>>(),
            vec![(7, "Mesa 7".into(), 4, DEFAULT_SECTION.into()), (8, "VIP 02".into(), 6, "Deck Externo".into())]
        );

        assert!(update_table(&db, &t1, TableInput { number: 8, capacity: 4, ..Default::default() }).await.is_err());
        update_table(&db, &t1, TableInput { number: 9, capacity: 2, section: Some("Varanda".into()), ..Default::default() })
            .await
            .unwrap();

        let (n, _) = rename_section(&db, "Deck Externo", "Deck Superior").await.unwrap();
        assert_eq!(n, 1);
        let (n, _) = move_to_section(&db, &[t1.clone(), t2.clone()], "Área VIP").await.unwrap();
        assert_eq!(n, 2);
        assert!(list_cards(&db).await.unwrap().iter().all(|c| c.section == "Área VIP"));

        open_table(&db, &t2, OpenTable::default()).await.unwrap();
        assert!(delete_table(&db, &t2).await.is_err(), "mesa ocupada não pode ser excluída");
        let (msg, _) = delete_table(&db, &t1).await.unwrap();
        assert_eq!(msg, "Mesa 9 excluída com sucesso");
    }
}
