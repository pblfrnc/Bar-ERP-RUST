//! Dia de movimento do bar: das 6h até as 5h59 do dia seguinte. As vendas fechadas na madrugada entram
//! no fechamento do dia anterior ("os fechamentos são até as 06h de cada dia", Pablo, 21/10/2026).
//! Vale para os relatórios de vendas (Dashboard, Comandas Finalizadas); o fiscal segue o dia do calendário.

use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};

/// Hora local em que começa o dia de movimento.
pub const START_HOUR: u32 = 6;

/// Dia de movimento de um instante (terça às 2h ainda é segunda).
pub fn day_of(dt: DateTime<Local>) -> NaiveDate {
    (dt.naive_local() - Duration::hours(i64::from(START_HOUR))).date()
}

/// Dia de movimento em andamento agora.
pub fn today() -> NaiveDate {
    day_of(Local::now())
}

/// Início e fim (exclusivo) do dia de movimento em ms UTC: das 6h do dia às 6h do dia seguinte.
pub fn bounds(day: NaiveDate) -> (i64, i64) {
    let at = |d: NaiveDate| {
        let naive = d.and_hms_opt(START_HOUR, 0, 0).expect("hora válida");
        Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|t| t.with_timezone(&Utc))
            // horário que não existe no fuso (mudança de horário): usa o de Brasília
            .unwrap_or_else(|| Utc.from_utc_datetime(&(naive + Duration::hours(3))))
    };
    let start = at(day);
    let end = day.succ_opt().map(at).unwrap_or(start + Duration::days(1));
    (start.timestamp_millis(), end.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(d: u32, h: u32, m: u32) -> DateTime<Local> {
        Local.from_local_datetime(&NaiveDate::from_ymd_opt(2026, 10, d).unwrap().and_hms_opt(h, m, 0).unwrap()).unwrap()
    }

    #[test]
    fn madrugada_entra_no_dia_anterior() {
        let segunda = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        assert_eq!(day_of(local(20, 2, 30)), segunda);
        assert_eq!(day_of(local(20, 5, 59)), segunda);
        assert_eq!(day_of(local(20, 6, 0)), NaiveDate::from_ymd_opt(2026, 10, 20).unwrap());
        assert_eq!(day_of(local(19, 23, 0)), segunda);
    }

    #[test]
    fn limites_das_6h_as_6h() {
        let segunda = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let (from, to) = bounds(segunda);
        assert_eq!(from, local(19, 6, 0).timestamp_millis());
        assert_eq!(to, local(20, 6, 0).timestamp_millis());
        let madrugada = local(20, 3, 0).timestamp_millis();
        assert!(from <= madrugada && madrugada < to);
    }
}
