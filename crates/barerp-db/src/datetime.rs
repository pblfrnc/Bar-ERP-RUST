//! Coluna `DATETIME` do `bar.db`: inteiro em milissegundos (Prisma) ou texto (`CURRENT_TIMESTAMP`).

use barerp_core::time;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use sqlx::{Decode, Encode, Type, TypeInfo, ValueRef};

/// Data/hora em UTC lida em qualquer um dos dois formatos e gravada sempre como o Prisma (inteiro).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DbTime(pub DateTime<Utc>);

impl DbTime {
    pub fn now() -> Self {
        Self(Utc::now())
    }
}

impl From<DateTime<Utc>> for DbTime {
    fn from(dt: DateTime<Utc>) -> Self {
        Self(dt)
    }
}

impl Type<Sqlite> for DbTime {
    fn type_info() -> SqliteTypeInfo {
        <i64 as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.name(), "INTEGER" | "TEXT" | "DATETIME" | "NUMERIC" | "REAL")
    }
}

impl<'r> Decode<'r, Sqlite> for DbTime {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        // Olha o tipo guardado na célula: decodificar texto como i64 daria só o ano
        let parsed = match value.type_info().name() {
            "TEXT" => {
                let s = <&str as Decode<Sqlite>>::decode(value)?;
                time::parse_text(s).ok_or_else(|| format!("data inválida no banco: {s:?}"))?
            }
            "REAL" => {
                let ms = <f64 as Decode<Sqlite>>::decode(value)? as i64;
                time::from_millis(ms).ok_or("data fora do intervalo")?
            }
            _ => {
                let ms = <i64 as Decode<Sqlite>>::decode(value)?;
                time::from_millis(ms).ok_or("data fora do intervalo")?
            }
        };
        Ok(Self(parsed))
    }
}

impl<'q> Encode<'q, Sqlite> for DbTime {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        <i64 as Encode<Sqlite>>::encode(time::to_db(self.0), buf)
    }
}
