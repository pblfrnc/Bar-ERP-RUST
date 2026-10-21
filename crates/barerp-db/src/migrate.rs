//! Migrações do `bar.db`.
//!
//! `sql/schema.sql` é o DDL gerado pelo Prisma a partir de `server/prisma/schema.prisma`. Todas as tabelas
//! e índices são `IF NOT EXISTS`, e as colunas que faltarem em bancos antigos são adicionadas lendo o
//! próprio schema — o mesmo que o `runMigrations.ts` fazia coluna por coluna com `ALTER TABLE`.
//!
//! Ficaram de fora os reparos de dados de uso único do Node (reclassificação da categoria IMPORTADOS e
//! recuperação da NF-e nº 1 série 2), que já rodaram no banco do cliente.

use sqlx::{Row, SqlitePool};

use barerp_core::{ids, time, vault};

const SCHEMA: &str = include_str!("../sql/schema.sql");

/// Permissões do administrador inicial (mesma lista do Node).
pub const ALL_PERMISSIONS: [&str; 9] =
    ["tables", "kds", "cash", "products", "fiscal", "settings", "dashboard", "customers", "suppliers"];

struct ColumnDef<'a> {
    table: &'a str,
    name: &'a str,
    definition: &'a str,
}

fn quoted(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some((&rest[..end], rest[end + 1..].trim()))
}

/// Colunas declaradas em cada `CREATE TABLE` do schema.
fn schema_columns() -> Vec<ColumnDef<'static>> {
    let mut columns = Vec::new();
    for statement in SCHEMA.split(';') {
        let Some(header_end) = statement.find('(') else { continue };
        let header = statement[..header_end].trim();
        let Some(after) = header.strip_prefix("CREATE TABLE IF NOT EXISTS ").or_else(|| header.strip_prefix("CREATE TABLE ")) else {
            continue;
        };
        let Some((table, _)) = quoted(after.trim()) else { continue };
        for line in statement[header_end + 1..].lines() {
            let line = line.trim().trim_end_matches(',');
            if let Some((name, definition)) = quoted(line) {
                columns.push(ColumnDef { table, name, definition });
            }
        }
    }
    columns
}

/// Definição aceita pelo `ALTER TABLE ADD COLUMN` do SQLite (sem chave primária, default constante,
/// e NOT NULL só com default).
fn alter_definition(definition: &str) -> Option<String> {
    if definition.contains("PRIMARY KEY") {
        return None;
    }
    let mut def = definition.replace("DEFAULT CURRENT_TIMESTAMP", "DEFAULT 0");
    if def.contains("NOT NULL") && !def.contains("DEFAULT") {
        def.push_str(if def.starts_with("TEXT") { " DEFAULT ''" } else { " DEFAULT 0" });
    }
    Some(def)
}

pub async fn run(pool: &SqlitePool) -> sqlx::Result<()> {
    // Tabelas e índices que faltarem
    sqlx::raw_sql(SCHEMA).execute(pool).await?;

    // Colunas que faltarem em bancos de versões antigas
    let mut existing: Vec<(String, String)> = Vec::new();
    for table in schema_columns().iter().map(|c| c.table).collect::<std::collections::BTreeSet<_>>() {
        let rows = sqlx::query(&format!("PRAGMA table_info(\"{table}\")")).fetch_all(pool).await?;
        existing.extend(rows.iter().map(|r| (table.to_string(), r.get::<String, _>("name"))));
    }
    for col in schema_columns() {
        if existing.iter().any(|(t, n)| t == col.table && n == col.name) {
            continue;
        }
        let Some(def) = alter_definition(col.definition) else { continue };
        tracing::info!("[migrações] adicionando coluna {}.{}", col.table, col.name);
        sqlx::query(&format!("ALTER TABLE \"{}\" ADD COLUMN \"{}\" {def}", col.table, col.name))
            .execute(pool)
            .await?;
    }

    // Bar e bebidas recebem a faixa de códigos 5001 se ainda estiverem no padrão genérico 1001
    sqlx::query(
        r#"UPDATE "Category" SET "codeStart" = 5001
           WHERE ("codeStart" = 1001 OR "codeStart" IS NULL)
             AND (LOWER("name") LIKE '%bar%' OR LOWER("name") LIKE '%cerveja%'
                  OR LOWER("name") LIKE '%chope%' OR LOWER("name") LIKE '%chopp%')"#,
    )
    .execute(pool)
    .await?;

    ensure_admin(pool).await
}

/// Administrador inicial (senha 1234) quando não há nenhum funcionário cadastrado.
async fn ensure_admin(pool: &SqlitePool) -> sqlx::Result<()> {
    let count: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "Staff""#).fetch_one(pool).await?;
    if count > 0 {
        return Ok(());
    }
    tracing::info!("[migrações] criando o usuário Administrador inicial (senha 1234)");
    let now = time::now_db();
    sqlx::query(
        r#"INSERT INTO "Staff" ("id", "name", "role", "password", "permissions", "active", "failedAttempts", "createdAt", "updatedAt")
           VALUES (?, 'Administrador', 'ADMIN', ?, ?, 1, 0, ?, ?)"#,
    )
    .bind(ids::cuid())
    .bind(vault::hash_password("1234"))
    .bind(serde_json::to_string(&ALL_PERMISSIONS).expect("lista fixa"))
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_table() {
        let cols = schema_columns();
        let tables: std::collections::BTreeSet<_> = cols.iter().map(|c| c.table).collect();
        assert_eq!(tables.len(), 25);
        assert!(cols.iter().any(|c| c.table == "Product" && c.name == "showInMenu"));
        assert!(cols.iter().any(|c| c.table == "ProductImage" && c.name == "data"));
    }

    #[test]
    fn alter_rules() {
        assert_eq!(alter_definition("DATETIME NOT NULL").unwrap(), "DATETIME NOT NULL DEFAULT 0");
        assert_eq!(alter_definition("TEXT NOT NULL").unwrap(), "TEXT NOT NULL DEFAULT ''");
        assert_eq!(
            alter_definition("DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP").unwrap(),
            "DATETIME NOT NULL DEFAULT 0"
        );
        assert_eq!(alter_definition("TEXT"), Some("TEXT".into()));
        assert_eq!(alter_definition("TEXT NOT NULL PRIMARY KEY"), None);
    }
}
