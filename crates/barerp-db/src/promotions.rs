//! Promoções do dia / happy hour: desconto em % ou preço fixo por dia da semana e horário.
//! Tradução das rotas `/products/promotions` de `server/src/routes/products.ts` (o cálculo do preço fica em
//! `barerp_core::pricing`).

use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::FromRow;

use barerp_core::ids;
use barerp_core::pricing::{is_promotion_active_at, Promotion};

use crate::events::Event;
use crate::{Db, DbError, DbResult, DbTime};

pub const WEEKDAY_LABELS: [&str; 7] = ["Dom", "Seg", "Ter", "Qua", "Qui", "Sex", "Sáb"];

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PromotionRow {
    pub id: String,
    pub name: String,
    #[sqlx(rename = "isActive")]
    pub is_active: bool,
    /// "1,2,3" (0 = domingo)
    #[sqlx(rename = "daysOfWeek")]
    pub days_of_week: String,
    #[sqlx(rename = "startTime")]
    pub start_time: String,
    #[sqlx(rename = "endTime")]
    pub end_time: String,
    /// PERCENT ou FIXED_PRICE
    #[sqlx(rename = "discountType")]
    pub discount_type: String,
    pub value: f64,
    /// JSON
    #[sqlx(rename = "productIds")]
    pub product_ids: String,
    /// JSON
    #[sqlx(rename = "categoryIds")]
    pub category_ids: String,
}

fn parse_ids(json: &str) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::Array(list)) => list
            .into_iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

impl PromotionRow {
    pub fn days(&self) -> Vec<u32> {
        self.days_of_week.split(',').filter_map(|d| d.trim().parse().ok()).filter(|d| *d <= 6).collect()
    }

    pub fn product_id_list(&self) -> Vec<String> {
        parse_ids(&self.product_ids)
    }

    pub fn category_id_list(&self) -> Vec<String> {
        parse_ids(&self.category_ids)
    }

    fn as_core(&self) -> Promotion {
        Promotion {
            id: self.id.clone(),
            name: self.name.clone(),
            is_active: self.is_active,
            days_of_week: self.days_of_week.clone(),
            start_time: self.start_time.clone(),
            end_time: self.end_time.clone(),
            discount_type: self.discount_type.clone(),
            value: self.value,
            product_ids: self.product_ids.clone(),
            category_ids: self.category_ids.clone(),
        }
    }

    /// Está valendo agora (selo "Agora" da lista).
    pub fn is_running_at(&self, now: NaiveDateTime) -> bool {
        is_promotion_active_at(&self.as_core(), now)
    }

    /// "25% off · Seg, Ter · 17:00–20:00" (`describePromotion`).
    pub fn describe(&self) -> String {
        let days = self.days();
        let days = if days.len() == 7 {
            "todos os dias".to_string()
        } else {
            days.iter().map(|d| WEEKDAY_LABELS[*d as usize]).collect::<Vec<_>>().join(", ")
        };
        let hours = if self.start_time == "00:00" && self.end_time == "23:59" {
            "o dia todo".to_string()
        } else {
            format!("{}–{}", self.start_time, self.end_time)
        };
        let value = if self.discount_type == "FIXED_PRICE" {
            format!("por R$ {:.2}", self.value)
        } else {
            let v = if self.value.fract() == 0.0 { format!("{}", self.value as i64) } else { self.value.to_string().replace('.', ",") };
            format!("{v}% off")
        };
        format!("{value} · {days} · {hours}")
    }
}

/// Ativas primeiro, depois por nome (como a lista do Node).
pub async fn list(db: &Db) -> DbResult<Vec<PromotionRow>> {
    Ok(sqlx::query_as(
        r#"SELECT "id", "name", "isActive", "daysOfWeek", "startTime", "endTime", "discountType", "value", "productIds", "categoryIds"
           FROM "Promotion" ORDER BY "isActive" DESC, "name""#,
    )
    .fetch_all(db.pool())
    .await?)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PromotionInput {
    pub name: String,
    pub is_active: bool,
    pub days: Vec<u32>,
    pub start_time: String,
    pub end_time: String,
    /// true = preço fixo; false = % de desconto
    pub fixed_price: bool,
    pub value: f64,
    pub product_ids: Vec<String>,
    pub category_ids: Vec<String>,
}

fn is_hhmm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && ((b[0] == b'2' && b[1] <= b'3') || b[0] <= b'1')
        && b[3] <= b'5'
}

fn event() -> Vec<Event> {
    vec![Event::CatalogUpdated { scope: "promotions" }]
}

/// Cria (`id = None`) ou altera, com as validações do `parsePromotionBody` do Node.
pub async fn save(db: &Db, id: Option<&str>, input: PromotionInput) -> DbResult<(String, Vec<Event>)> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(DbError::rule("Informe o nome da promoção (ex: Happy Hour do Chopp)."));
    }
    if input.value.is_nan() || input.value <= 0.0 {
        return Err(DbError::rule("Informe o valor da promoção."));
    }
    if !input.fixed_price && input.value >= 100.0 {
        return Err(DbError::rule("O desconto em % precisa ser menor que 100."));
    }
    let mut days: Vec<u32> = input.days.iter().copied().filter(|d| *d <= 6).collect();
    days.sort();
    days.dedup();
    if days.is_empty() {
        return Err(DbError::rule("Escolha pelo menos um dia da semana."));
    }
    if !is_hhmm(&input.start_time) || !is_hhmm(&input.end_time) {
        return Err(DbError::rule("Horário inválido. Use o formato HH:MM."));
    }
    if input.start_time == input.end_time {
        return Err(DbError::rule("Início e fim iguais. Para o dia todo, use 00:00 até 23:59."));
    }
    if input.product_ids.is_empty() && input.category_ids.is_empty() {
        return Err(DbError::rule("Escolha os produtos ou categorias da promoção."));
    }
    let days = days.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let products = serde_json::to_string(&input.product_ids).expect("lista de textos");
    let categories = serde_json::to_string(&input.category_ids).expect("lista de textos");
    let kind = if input.fixed_price { "FIXED_PRICE" } else { "PERCENT" };
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;
    let id = match id {
        Some(id) => {
            let changed = sqlx::query(
                r#"UPDATE "Promotion" SET "name" = ?, "isActive" = ?, "daysOfWeek" = ?, "startTime" = ?, "endTime" = ?,
                       "discountType" = ?, "value" = ?, "productIds" = ?, "categoryIds" = ?, "updatedAt" = ? WHERE "id" = ?"#,
            )
            .bind(name)
            .bind(input.is_active)
            .bind(&days)
            .bind(&input.start_time)
            .bind(&input.end_time)
            .bind(kind)
            .bind(input.value)
            .bind(&products)
            .bind(&categories)
            .bind(now)
            .bind(id)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            if changed == 0 {
                return Err(DbError::not_found("Promoção não encontrada"));
            }
            id.to_string()
        }
        None => {
            let id = ids::cuid();
            sqlx::query(
                r#"INSERT INTO "Promotion" ("id", "name", "isActive", "daysOfWeek", "startTime", "endTime", "discountType", "value",
                                            "productIds", "categoryIds", "createdAt", "updatedAt")
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(name)
            .bind(input.is_active)
            .bind(&days)
            .bind(&input.start_time)
            .bind(&input.end_time)
            .bind(kind)
            .bind(input.value)
            .bind(&products)
            .bind(&categories)
            .bind(now)
            .bind(now)
            .execute(&mut **tx)
            .await?;
            id
        }
    };
    tx.commit().await?;
    Ok((id, event()))
}

/// Liga/desliga sem mexer no resto (chave da lista).
pub async fn set_active(db: &Db, id: &str, active: bool) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let changed = sqlx::query(r#"UPDATE "Promotion" SET "isActive" = ?, "updatedAt" = ? WHERE "id" = ?"#)
        .bind(active)
        .bind(DbTime::now())
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(DbError::not_found("Promoção não encontrada"));
    }
    tx.commit().await?;
    Ok(event())
}

pub async fn delete(db: &Db, id: &str) -> DbResult<Vec<Event>> {
    let mut tx = db.begin_write().await?;
    let removed = sqlx::query(r#"DELETE FROM "Promotion" WHERE "id" = ?"#).bind(id).execute(&mut **tx).await?;
    if removed.rows_affected() == 0 {
        return Err(DbError::not_found("Promoção não encontrada"));
    }
    tx.commit().await?;
    Ok(event())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn happy() -> PromotionInput {
        PromotionInput {
            name: " Happy Hour ".into(),
            is_active: true,
            days: vec![5, 1, 1, 9],
            start_time: "17:00".into(),
            end_time: "20:00".into(),
            fixed_price: false,
            value: 25.0,
            product_ids: vec!["p1".into()],
            category_ids: vec![],
        }
    }

    #[tokio::test]
    async fn cria_lista_descreve_e_valida() {
        let db = Db::open_in_memory().await.unwrap();
        let (id, ev) = save(&db, None, happy()).await.unwrap();
        assert_eq!(ev, vec![Event::CatalogUpdated { scope: "promotions" }]);
        let p = &list(&db).await.unwrap()[0];
        assert_eq!((p.name.as_str(), p.days_of_week.as_str()), ("Happy Hour", "1,5"));
        assert_eq!(p.describe(), "25% off · Seg, Sex · 17:00–20:00");
        assert_eq!(p.product_id_list(), vec!["p1"]);
        // Segunda 18h
        let mon = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap().and_hms_opt(18, 0, 0).unwrap();
        assert!(p.is_running_at(mon));
        assert!(!p.is_running_at(mon + chrono::Duration::hours(3)));

        let mut fixed = happy();
        fixed.fixed_price = true;
        fixed.value = 9.9;
        fixed.days = (0..7).collect();
        fixed.start_time = "00:00".into();
        fixed.end_time = "23:59".into();
        save(&db, Some(&id), fixed).await.unwrap();
        assert_eq!(list(&db).await.unwrap()[0].describe(), "por R$ 9.90 · todos os dias · o dia todo");

        set_active(&db, &id, false).await.unwrap();
        assert!(!list(&db).await.unwrap()[0].is_active);

        let err = |f: fn(&mut PromotionInput)| {
            let mut p = happy();
            f(&mut p);
            p
        };
        for bad in [
            err(|p| p.name = " ".into()),
            err(|p| p.value = 0.0),
            err(|p| p.value = 100.0),
            err(|p| p.days = vec![]),
            err(|p| p.start_time = "24:00".into()),
            err(|p| p.end_time = "17:00".into()),
            err(|p| p.product_ids = vec![]),
        ] {
            assert!(matches!(save(&db, None, bad).await, Err(DbError::Rule(_))));
        }
        delete(&db, &id).await.unwrap();
        assert!(matches!(delete(&db, &id).await, Err(DbError::NotFound(_))));
    }
}
