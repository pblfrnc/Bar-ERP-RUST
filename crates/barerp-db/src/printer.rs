//! Configurações de impressoras térmicas (caixa e cozinha): modelo, largura do papel (80mm/58mm),
//! corte automático e destinos de rede (IP) ou spooler Windows.
//! Tradução de `server/src/routes/settings.ts`.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::{Db, DbResult, DbTime};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PrinterSettings {
    #[serde(default)]
    pub id: String,
    #[sqlx(rename = "cashierPrinter")]
    pub cashier_printer: Option<String>,
    #[sqlx(rename = "kitchenPrinter")]
    pub kitchen_printer: Option<String>,
    #[sqlx(rename = "paperWidth")]
    pub paper_width: f64,
    #[sqlx(rename = "marginTop")]
    pub margin_top: f64,
    #[sqlx(rename = "marginBottom")]
    pub margin_bottom: f64,
    #[sqlx(rename = "marginLeft")]
    pub margin_left: f64,
    #[sqlx(rename = "marginRight")]
    pub margin_right: f64,
    #[sqlx(rename = "fontScale")]
    pub font_scale: f64,
    #[sqlx(rename = "qrSize")]
    pub qr_size: f64,
    #[sqlx(rename = "autoCut")]
    pub auto_cut: bool,
    #[sqlx(rename = "silentPrint")]
    pub silent_print: bool,
    pub copies: i64,
    #[sqlx(rename = "extraFeedLines")]
    pub extra_feed_lines: i64,
    #[sqlx(rename = "printLogo")]
    pub print_logo: bool,
    #[sqlx(rename = "updatedAt")]
    #[serde(default)]
    pub updated_at: Option<DbTime>,
}

impl Default for PrinterSettings {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            cashier_printer: None,
            kitchen_printer: None,
            paper_width: 80.0,
            margin_top: 2.0,
            margin_bottom: 12.0,
            margin_left: 1.0,
            margin_right: 1.0,
            font_scale: 100.0,
            qr_size: 100.0,
            auto_cut: true,
            silent_print: true,
            copies: 1,
            extra_feed_lines: 3,
            print_logo: true,
            updated_at: None,
        }
    }
}

async fn ensure_table(conn: &mut sqlx::SqliteConnection) -> DbResult<()> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS "PrinterSettings" (
            "id" TEXT NOT NULL PRIMARY KEY DEFAULT 'default',
            "cashierPrinter" TEXT,
            "kitchenPrinter" TEXT,
            "paperWidth" REAL DEFAULT 80,
            "marginTop" REAL DEFAULT 2,
            "marginBottom" REAL DEFAULT 12,
            "marginLeft" REAL DEFAULT 1,
            "marginRight" REAL DEFAULT 1,
            "fontScale" REAL DEFAULT 100,
            "qrSize" REAL DEFAULT 100,
            "autoCut" BOOLEAN DEFAULT 1,
            "silentPrint" BOOLEAN DEFAULT 1,
            "copies" INTEGER DEFAULT 1,
            "extraFeedLines" INTEGER DEFAULT 3,
            "printLogo" BOOLEAN DEFAULT 1,
            "updatedAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )"#,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn get_printer_settings(db: &Db) -> DbResult<PrinterSettings> {
    let mut conn = db.pool().acquire().await?;
    let _ = ensure_table(&mut conn).await;
    let row: Option<PrinterSettings> = sqlx::query_as(r#"SELECT * FROM "PrinterSettings" WHERE "id" = 'default' LIMIT 1"#)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.unwrap_or_default())
}

pub async fn save_printer_settings(db: &Db, s: &PrinterSettings) -> DbResult<PrinterSettings> {
    let mut tx = db.begin_write().await?;
    ensure_table(&mut tx).await?;
    let now = DbTime::now();
    sqlx::query(
        r#"INSERT INTO "PrinterSettings" (
            "id", "cashierPrinter", "kitchenPrinter", "paperWidth",
            "marginTop", "marginBottom", "marginLeft", "marginRight",
            "fontScale", "qrSize", "autoCut", "silentPrint",
            "copies", "extraFeedLines", "printLogo", "updatedAt"
        ) VALUES ('default', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT("id") DO UPDATE SET
            "cashierPrinter" = excluded."cashierPrinter",
            "kitchenPrinter" = excluded."kitchenPrinter",
            "paperWidth" = excluded."paperWidth",
            "marginTop" = excluded."marginTop",
            "marginBottom" = excluded."marginBottom",
            "marginLeft" = excluded."marginLeft",
            "marginRight" = excluded."marginRight",
            "fontScale" = excluded."fontScale",
            "qrSize" = excluded."qrSize",
            "autoCut" = excluded."autoCut",
            "silentPrint" = excluded."silentPrint",
            "copies" = excluded."copies",
            "extraFeedLines" = excluded."extraFeedLines",
            "printLogo" = excluded."printLogo",
            "updatedAt" = excluded."updatedAt""#,
    )
    .bind(&s.cashier_printer)
    .bind(&s.kitchen_printer)
    .bind(s.paper_width)
    .bind(s.margin_top)
    .bind(s.margin_bottom)
    .bind(s.margin_left)
    .bind(s.margin_right)
    .bind(s.font_scale)
    .bind(s.qr_size)
    .bind(s.auto_cut)
    .bind(s.silent_print)
    .bind(s.copies)
    .bind(s.extra_feed_lines)
    .bind(s.print_logo)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    tx.commit().await?;
    get_printer_settings(db).await
}

/// Configuração que o Electron guardava em `printer-settings.json` (no userData, ao lado do `bar.db`). A tela do
/// Node gravava no banco e nesse arquivo, mas lia primeiro o banco: um PC que só tem o arquivo perderia a impressora.
pub fn from_electron_json(v: &serde_json::Value) -> PrinterSettings {
    let d = PrinterSettings::default();
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let num = |k: &str, default: f64| v.get(k).and_then(|x| x.as_f64()).unwrap_or(default);
    let flag = |k: &str, default: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(default);
    PrinterSettings {
        cashier_printer: text("cashierPrinter"),
        kitchen_printer: text("kitchenPrinter"),
        paper_width: Some(num("paperWidth", d.paper_width)).filter(|w| *w > 0.0).unwrap_or(d.paper_width),
        margin_top: num("marginTop", d.margin_top),
        margin_bottom: num("marginBottom", d.margin_bottom),
        margin_left: num("marginLeft", d.margin_left),
        margin_right: num("marginRight", d.margin_right),
        font_scale: num("fontScale", d.font_scale),
        qr_size: num("qrSize", d.qr_size),
        auto_cut: flag("autoCut", d.auto_cut),
        silent_print: flag("silentPrint", d.silent_print),
        copies: (num("copies", d.copies as f64) as i64).max(1),
        extra_feed_lines: (num("extraFeedLines", d.extra_feed_lines as f64) as i64).max(0),
        print_logo: flag("printLogo", d.print_logo),
        ..d
    }
}

/// Traz o `printer-settings.json` do Electron para o banco, só quando o banco ainda não tem configuração de
/// impressora (uma vez só: depois disso vale o que for salvo na tela). Devolve se importou.
pub async fn import_electron_file(db: &Db, path: &std::path::Path) -> DbResult<bool> {
    if get_printer_settings(db).await?.updated_at.is_some() {
        return Ok(false);
    }
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(false) };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return Ok(false) };
    save_printer_settings(db, &from_electron_json(&json)).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn printer_settings_crud() {
        let db = Db::open_in_memory().await.unwrap();
        let default = get_printer_settings(&db).await.unwrap();
        assert_eq!(default.paper_width, 80.0);
        assert_eq!(default.cashier_printer, None);

        let mut custom = default.clone();
        custom.cashier_printer = Some("192.168.1.100:9100".to_string());
        custom.paper_width = 58.0;
        let saved = save_printer_settings(&db, &custom).await.unwrap();
        assert_eq!(saved.cashier_printer, Some("192.168.1.100:9100".to_string()));
        assert_eq!(saved.paper_width, 58.0);

        let read_again = get_printer_settings(&db).await.unwrap();
        assert_eq!(read_again.cashier_printer, Some("192.168.1.100:9100".to_string()));
        assert_eq!(read_again.paper_width, 58.0);
    }

    #[tokio::test]
    async fn electron_printer_file_imported_once() {
        let db = Db::open_in_memory().await.unwrap();
        let dir = std::env::temp_dir().join(format!("barerp-printer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("printer-settings.json");
        std::fs::write(&file, r#"{"cashierPrinter":"POS-80","kitchenPrinter":"","paperWidth":58,"copies":2,"autoCut":false}"#).unwrap();

        assert!(import_electron_file(&db, &file).await.unwrap());
        let s = get_printer_settings(&db).await.unwrap();
        assert_eq!(s.cashier_printer.as_deref(), Some("POS-80"));
        assert_eq!(s.kitchen_printer, None);
        assert_eq!((s.paper_width, s.copies, s.auto_cut, s.margin_bottom), (58.0, 2, false, 12.0));

        // Depois de salva no banco, a configuração da tela manda (o arquivo não é lido de novo)
        let mut changed = s.clone();
        changed.cashier_printer = None;
        save_printer_settings(&db, &changed).await.unwrap();
        assert!(!import_electron_file(&db, &file).await.unwrap());
        assert_eq!(get_printer_settings(&db).await.unwrap().cashier_printer, None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
