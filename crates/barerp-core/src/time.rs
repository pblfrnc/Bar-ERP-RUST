//! Datas no formato do `bar.db`.
//!
//! O Prisma grava `DateTime` como inteiro (milissegundos desde 1970, UTC). As linhas inseridas com SQL
//! puro pelo `runMigrations.ts` (ex: o administrador inicial) ficaram com `CURRENT_TIMESTAMP`, que é
//! texto `AAAA-MM-DD HH:MM:SS` em UTC. Lemos os dois e gravamos sempre o inteiro, como o Prisma.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};

/// Converte para o inteiro que o Prisma grava.
pub fn to_db(dt: DateTime<Utc>) -> i64 {
    dt.timestamp_millis()
}

/// Agora, no formato do banco.
pub fn now_db() -> i64 {
    to_db(Utc::now())
}

pub fn from_millis(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

/// Lê uma data gravada como texto (CURRENT_TIMESTAMP, ISO 8601 ou número em texto).
pub fn parse_text(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(ms) = s.parse::<i64>() {
        return from_millis(ms);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
        .map(|naive| Utc.from_utc_datetime(&naive))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_formats() {
        let prisma = from_millis(1_792_337_230_588).unwrap();
        assert_eq!(prisma.to_rfc3339(), "2026-10-18T15:27:10.588+00:00");
        assert_eq!(parse_text("2026-10-18 15:27:10").unwrap().timestamp(), prisma.timestamp());
        assert_eq!(parse_text("1792337230588"), Some(prisma));
        assert_eq!(parse_text("2026-10-18T12:27:10.588-03:00"), Some(prisma));
        assert_eq!(parse_text("lixo"), None);
        assert_eq!(to_db(prisma), 1_792_337_230_588);
    }
}
