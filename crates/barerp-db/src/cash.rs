//! Controle de turnos de caixa (abertura, sangria, suprimento, fechamento cego e reabertura).
//! Tradução de `server/src/routes/cash.ts`.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection};

use barerp_core::ids;
use barerp_core::pricing::round2;

use crate::events::Event;
use crate::{Db, DbError, DbResult, DbTime};

pub const TX_SUPPLY: &str = "SUPPLY";
pub const TX_WITHDRAWAL: &str = "WITHDRAWAL";

/// Turno de caixa gravado no banco.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CashShift {
    pub id: String,
    #[sqlx(rename = "openedAt")]
    pub opened_at: DbTime,
    #[sqlx(rename = "closedAt")]
    pub closed_at: Option<DbTime>,
    #[sqlx(rename = "openedBy")]
    pub opened_by: String,
    #[sqlx(rename = "closedBy")]
    pub closed_by: Option<String>,
    #[sqlx(rename = "initialBalance")]
    pub initial_balance: f64,
    #[sqlx(rename = "finalBalance")]
    pub final_balance: Option<f64>,
    #[sqlx(rename = "expectedBalance")]
    pub expected_balance: Option<f64>,
    pub difference: Option<f64>,
    pub status: String,
    pub notes: Option<String>,
}

/// Movimentação avulsa de caixa (Sangria ou Suprimento).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CashTransaction {
    pub id: String,
    #[sqlx(rename = "cashShiftId")]
    pub cash_shift_id: String,
    pub r#type: String, // "SUPPLY" ou "WITHDRAWAL"
    pub amount: f64,
    pub reason: String,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
}

/// Pagamento registrado no turno.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ShiftPayment {
    pub id: String,
    #[sqlx(rename = "orderId")]
    pub order_id: String,
    pub amount: f64,
    pub method: String,
    #[sqlx(rename = "cashShiftId")]
    pub cash_shift_id: Option<String>,
    pub notes: Option<String>,
    #[sqlx(rename = "receivedAt")]
    pub received_at: DbTime,
}

/// Comissão / taxa de 10% acumulada por garçom no turno.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaiterCommission {
    pub waiter_id: Option<String>,
    pub waiter_name: String,
    pub orders_count: i64,
    pub total_sales: f64,
    pub service_fee_total: f64,
}

/// Resumo financeiro do turno.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CashSummary {
    pub initial_balance: f64,
    pub total_supplies: f64,
    pub total_withdrawals: f64,
    pub total_sales: f64,
    pub payments_by_method: std::collections::HashMap<String, f64>,
    pub expected_cash_in_drawer: f64,
    pub total_service_fees_shift: f64,
    pub waiter_commissions: Vec<WaiterCommission>,
}

/// Status do caixa atual com o turno ativo (se houver) e o resumo financeiro.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentShiftState {
    pub is_open: bool,
    pub shift: Option<CashShift>,
    pub summary: Option<CashSummary>,
}

/// Relatório emitido no fechamento de caixa.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedShiftReport {
    pub expected_cash: f64,
    pub counted_cash: f64,
    pub difference: f64,
    pub total_service_fees_shift: f64,
    pub waiter_commissions: Vec<WaiterCommission>,
}

/// Dados para abertura de turno.
#[derive(Debug, Clone, Default)]
pub struct OpenShiftInput {
    pub initial_balance: f64,
    pub opened_by: Option<String>,
    pub notes: Option<String>,
}

/// Dados para movimentação avulsa.
#[derive(Debug, Clone)]
pub struct TransactionInput {
    pub tx_type: String, // "SUPPLY" | "WITHDRAWAL"
    pub amount: f64,
    pub reason: String,
}

/// Dados para fechamento de caixa.
#[derive(Debug, Clone)]
pub struct CloseShiftInput {
    pub final_cash_count: f64,
    pub closed_by: Option<String>,
    pub notes: Option<String>,
}

/// Consulta o turno de caixa atualmente aberto.
pub async fn current_shift(db: &Db) -> DbResult<CurrentShiftState> {
    let pool = db.pool();
    let shift: Option<CashShift> = sqlx::query_as(
        r#"SELECT "id", "openedAt", "closedAt", "openedBy", "closedBy",
                  "initialBalance", "finalBalance", "expectedBalance",
                  "difference", "status", "notes"
           FROM "CashShift"
           WHERE "status" = 'OPEN'
           ORDER BY "openedAt" DESC LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await?;

    let Some(shift) = shift else {
        return Ok(CurrentShiftState { is_open: false, shift: None, summary: None });
    };

    let summary = calculate_shift_summary(pool, &shift).await?;
    Ok(CurrentShiftState {
        is_open: true,
        shift: Some(shift),
        summary: Some(summary),
    })
}

/// Abre um novo turno de caixa.
pub async fn open_shift(db: &Db, input: OpenShiftInput) -> DbResult<(CashShift, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let active: Option<String> = sqlx::query_scalar(
        r#"SELECT "id" FROM "CashShift" WHERE "status" = 'OPEN' LIMIT 1"#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    if active.is_some() {
        return Err(DbError::Rule("Já existe um turno de caixa aberto".into()));
    }

    let id = ids::cuid();
    let now = DbTime::now();
    let opened_by = input
        .opened_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Operador de Caixa");
    let initial_balance = round2(input.initial_balance.max(0.0));
    let notes = input.notes.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    sqlx::query(
        r#"INSERT INTO "CashShift" ("id", "openedAt", "openedBy", "initialBalance", "status", "notes")
           VALUES (?, ?, ?, ?, 'OPEN', ?)"#,
    )
    .bind(&id)
    .bind(now)
    .bind(opened_by)
    .bind(initial_balance)
    .bind(&notes)
    .execute(&mut **tx)
    .await?;

    let shift = CashShift {
        id,
        opened_at: now,
        closed_at: None,
        opened_by: opened_by.to_string(),
        closed_by: None,
        initial_balance,
        final_balance: None,
        expected_balance: None,
        difference: None,
        status: "OPEN".to_string(),
        notes,
    };

    tx.commit().await?;
    Ok((shift, vec![Event::CashUpdated]))
}

/// Registra uma movimentação avulsa de caixa (Sangria ou Suprimento).
pub async fn add_transaction(
    db: &Db,
    input: TransactionInput,
) -> DbResult<(CashTransaction, Vec<Event>)> {
    let tx_type = input.tx_type.trim().to_uppercase();
    if tx_type != "SUPPLY" && tx_type != "WITHDRAWAL" {
        return Err(DbError::Rule("Tipo de movimentação inválido (deve ser SUPPLY ou WITHDRAWAL)".into()));
    }
    let amount = round2(input.amount);
    if amount <= 0.0 {
        return Err(DbError::Rule("Valor da movimentação deve ser maior que zero".into()));
    }
    let reason = input.reason.trim();
    if reason.is_empty() {
        return Err(DbError::Rule("Motivo da movimentação é obrigatório".into()));
    }

    let mut tx = db.begin_write().await?;
    let active_id: Option<String> = sqlx::query_scalar(
        r#"SELECT "id" FROM "CashShift" WHERE "status" = 'OPEN' ORDER BY "openedAt" DESC LIMIT 1"#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    let Some(cash_shift_id) = active_id else {
        return Err(DbError::Rule("Nenhum turno de caixa aberto".into()));
    };

    let id = ids::cuid();
    let now = DbTime::now();

    sqlx::query(
        r#"INSERT INTO "CashTransaction" ("id", "cashShiftId", "type", "amount", "reason", "createdAt")
           VALUES (?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(&cash_shift_id)
    .bind(&tx_type)
    .bind(amount)
    .bind(reason)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let transaction = CashTransaction {
        id,
        cash_shift_id,
        r#type: tx_type,
        amount,
        reason: reason.to_string(),
        created_at: now,
    };

    tx.commit().await?;
    Ok((transaction, vec![Event::CashUpdated]))
}

/// Fecha o turno de caixa ativo (Fechamento Cego).
pub async fn close_shift(
    db: &Db,
    input: CloseShiftInput,
) -> DbResult<(CashShift, ClosedShiftReport, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let shift: Option<CashShift> = sqlx::query_as(
        r#"SELECT "id", "openedAt", "closedAt", "openedBy", "closedBy",
                  "initialBalance", "finalBalance", "expectedBalance",
                  "difference", "status", "notes"
           FROM "CashShift"
           WHERE "status" = 'OPEN'
           ORDER BY "openedAt" DESC LIMIT 1"#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    let Some(shift) = shift else {
        return Err(DbError::Rule("Nenhum turno aberto para fechar".into()));
    };

    // Calcular suprimentos e sangrias
    let (supplies, withdrawals): (f64, f64) = sqlx::query_as(
        r#"SELECT
             COALESCE(SUM(CASE WHEN "type" = 'SUPPLY' THEN "amount" ELSE 0 END), 0.0),
             COALESCE(SUM(CASE WHEN "type" = 'WITHDRAWAL' THEN "amount" ELSE 0 END), 0.0)
           FROM "CashTransaction"
           WHERE "cashShiftId" = ?"#,
    )
    .bind(&shift.id)
    .fetch_one(&mut **tx)
    .await?;

    // Vendas em dinheiro
    let cash_sales: f64 = sqlx::query_scalar(
        r#"SELECT COALESCE(SUM("amount"), 0.0)
           FROM "Payment"
           WHERE "cashShiftId" = ? AND "method" = 'CASH'"#,
    )
    .bind(&shift.id)
    .fetch_one(&mut **tx)
    .await?;

    let expected_cash = round2(shift.initial_balance + supplies - withdrawals + cash_sales);
    let counted_cash = round2(input.final_cash_count.max(0.0));
    let difference = round2(counted_cash - expected_cash);

    let waiter_commissions = load_waiter_commissions(&mut tx, &shift.id).await?;
    let total_service_fees_shift: f64 = waiter_commissions.iter().map(|w| w.service_fee_total).sum();

    let now = DbTime::now();
    let closed_by = input
        .closed_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Operador de Caixa");
    let notes = input
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(shift.notes.clone());

    sqlx::query(
        r#"UPDATE "CashShift"
           SET "status" = 'CLOSED',
               "closedAt" = ?,
               "closedBy" = ?,
               "finalBalance" = ?,
               "expectedBalance" = ?,
               "difference" = ?,
               "notes" = ?
           WHERE "id" = ?"#,
    )
    .bind(now)
    .bind(closed_by)
    .bind(counted_cash)
    .bind(expected_cash)
    .bind(difference)
    .bind(&notes)
    .bind(&shift.id)
    .execute(&mut **tx)
    .await?;

    let closed_shift = CashShift {
        id: shift.id,
        opened_at: shift.opened_at,
        closed_at: Some(now),
        opened_by: shift.opened_by,
        closed_by: Some(closed_by.to_string()),
        initial_balance: shift.initial_balance,
        final_balance: Some(counted_cash),
        expected_balance: Some(expected_cash),
        difference: Some(difference),
        status: "CLOSED".to_string(),
        notes,
    };

    let report = ClosedShiftReport {
        expected_cash,
        counted_cash,
        difference,
        total_service_fees_shift: round2(total_service_fees_shift),
        waiter_commissions,
    };

    tx.commit().await?;
    Ok((closed_shift, report, vec![Event::CashUpdated]))
}

/// Reabre o último turno de caixa fechado para correções de digitação.
pub async fn reopen_last_shift(db: &Db) -> DbResult<(CashShift, Vec<Event>)> {
    let mut tx = db.begin_write().await?;
    let active: Option<String> = sqlx::query_scalar(
        r#"SELECT "id" FROM "CashShift" WHERE "status" = 'OPEN' LIMIT 1"#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    if active.is_some() {
        return Err(DbError::Rule("Já existe um turno de caixa aberto no momento.".into()));
    }

    let last: Option<CashShift> = sqlx::query_as(
        r#"SELECT "id", "openedAt", "closedAt", "openedBy", "closedBy",
                  "initialBalance", "finalBalance", "expectedBalance",
                  "difference", "status", "notes"
           FROM "CashShift"
           WHERE "status" = 'CLOSED'
           ORDER BY "closedAt" DESC LIMIT 1"#,
    )
    .fetch_optional(&mut **tx)
    .await?;

    let Some(last) = last else {
        return Err(DbError::not_found("Nenhum turno fechado encontrado para reabrir."));
    };

    sqlx::query(
        r#"UPDATE "CashShift"
           SET "status" = 'OPEN',
               "closedAt" = NULL,
               "closedBy" = NULL,
               "finalBalance" = NULL,
               "difference" = NULL
           WHERE "id" = ?"#,
    )
    .bind(&last.id)
    .execute(&mut **tx)
    .await?;

    let reopened = CashShift {
        id: last.id,
        opened_at: last.opened_at,
        closed_at: None,
        opened_by: last.opened_by,
        closed_by: None,
        initial_balance: last.initial_balance,
        final_balance: None,
        expected_balance: None,
        difference: None,
        status: "OPEN".to_string(),
        notes: last.notes,
    };

    tx.commit().await?;
    Ok((reopened, vec![Event::CashUpdated]))
}

/// Histórico dos últimos turnos de caixa fechados ou abertos.
pub async fn shift_history(db: &Db, limit: i64) -> DbResult<Vec<CashShift>> {
    let limit = limit.clamp(1, 100);
    let shifts: Vec<CashShift> = sqlx::query_as(
        r#"SELECT "id", "openedAt", "closedAt", "openedBy", "closedBy",
                  "initialBalance", "finalBalance", "expectedBalance",
                  "difference", "status", "notes"
           FROM "CashShift"
           ORDER BY "openedAt" DESC LIMIT ?"#,
    )
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(shifts)
}

/// Movimentações avulsas (sangrias/suprimentos) de um turno.
pub async fn list_transactions(db: &Db, shift_id: &str) -> DbResult<Vec<CashTransaction>> {
    let txs: Vec<CashTransaction> = sqlx::query_as(
        r#"SELECT "id", "cashShiftId", "type", "amount", "reason", "createdAt"
           FROM "CashTransaction"
           WHERE "cashShiftId" = ?
           ORDER BY "createdAt" DESC"#,
    )
    .bind(shift_id)
    .fetch_all(db.pool())
    .await?;
    Ok(txs)
}

/// Calcula o resumo de movimentações e comissões para um turno.
async fn calculate_shift_summary(
    pool: &sqlx::SqlitePool,
    shift: &CashShift,
) -> DbResult<CashSummary> {
    let (supplies, withdrawals): (f64, f64) = sqlx::query_as(
        r#"SELECT
             COALESCE(SUM(CASE WHEN "type" = 'SUPPLY' THEN "amount" ELSE 0 END), 0.0),
             COALESCE(SUM(CASE WHEN "type" = 'WITHDRAWAL' THEN "amount" ELSE 0 END), 0.0)
           FROM "CashTransaction"
           WHERE "cashShiftId" = ?"#,
    )
    .bind(&shift.id)
    .fetch_one(pool)
    .await?;

    let payment_rows: Vec<(String, f64)> = sqlx::query_as(
        r#"SELECT "method", COALESCE(SUM("amount"), 0.0)
           FROM "Payment"
           WHERE "cashShiftId" = ?
           GROUP BY "method""#,
    )
    .bind(&shift.id)
    .fetch_all(pool)
    .await?;

    let mut payments_by_method = std::collections::HashMap::from([
        ("CASH".to_string(), 0.0),
        ("CREDIT_CARD".to_string(), 0.0),
        ("DEBIT_CARD".to_string(), 0.0),
        ("PIX".to_string(), 0.0),
        ("VOUCHER".to_string(), 0.0),
    ]);

    let mut total_sales = 0.0;
    for (method, amount) in payment_rows {
        total_sales += amount;
        payments_by_method.insert(method, round2(amount));
    }

    let mut conn = pool.acquire().await?;
    let waiter_commissions = load_waiter_commissions(&mut conn, &shift.id).await?;
    let total_service_fees_shift: f64 = waiter_commissions.iter().map(|w| w.service_fee_total).sum();

    let cash_in_drawer = shift.initial_balance + payments_by_method.get("CASH").copied().unwrap_or(0.0) + supplies - withdrawals;

    Ok(CashSummary {
        initial_balance: shift.initial_balance,
        total_supplies: round2(supplies),
        total_withdrawals: round2(withdrawals),
        total_sales: round2(total_sales),
        payments_by_method,
        expected_cash_in_drawer: round2(cash_in_drawer),
        total_service_fees_shift: round2(total_service_fees_shift),
        waiter_commissions,
    })
}

#[derive(FromRow)]
struct OrderFeeRow {
    #[sqlx(rename = "waiterId")]
    waiter_id: Option<String>,
    #[sqlx(rename = "waiterName")]
    waiter_name: Option<String>,
    total: f64,
    #[sqlx(rename = "isServiceFeeActive")]
    is_service_fee_active: bool,
    #[sqlx(rename = "serviceFee")]
    service_fee: f64,
}

/// Carrega comissões e totais de pedidos agrupados por garçom para o turno especificado.
async fn load_waiter_commissions(
    conn: &mut SqliteConnection,
    shift_id: &str,
) -> DbResult<Vec<WaiterCommission>> {
    // Busca pedidos distintos que receberam pagamentos neste turno
    let orders: Vec<OrderFeeRow> = sqlx::query_as(
        r#"SELECT DISTINCT o."id", o."waiterId", o."waiterName", o."total", o."isServiceFeeActive", o."serviceFee"
           FROM "Order" o
           JOIN "Payment" p ON p."orderId" = o."id"
           WHERE p."cashShiftId" = ?"#,
    )
    .bind(shift_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut map: std::collections::HashMap<String, WaiterCommission> = std::collections::HashMap::new();

    for o in orders {
        let waiter_key = o
            .waiter_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Garçom Geral")
            .to_string();

        let entry = map.entry(waiter_key.clone()).or_insert_with(|| WaiterCommission {
            waiter_id: o.waiter_id,
            waiter_name: waiter_key,
            orders_count: 0,
            total_sales: 0.0,
            service_fee_total: 0.0,
        });

        entry.orders_count += 1;
        entry.total_sales += o.total;
        if o.is_service_fee_active {
            entry.service_fee_total += o.service_fee;
        }
    }

    let mut list: Vec<WaiterCommission> = map
        .into_values()
        .map(|mut w| {
            w.total_sales = round2(w.total_sales);
            w.service_fee_total = round2(w.service_fee_total);
            w
        })
        .collect();

    list.sort_by(|a, b| b.service_fee_total.partial_cmp(&a.service_fee_total).unwrap_or(std::cmp::Ordering::Equal));
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shift_flow_open_transaction_close_reopen() {
        let db = Db::open_in_memory().await.unwrap();

        let initial_state = current_shift(&db).await.unwrap();
        assert!(!initial_state.is_open);
        assert!(initial_state.shift.is_none());

        // Abertura
        let (shift, events) = open_shift(
            &db,
            OpenShiftInput {
                initial_balance: 150.0,
                opened_by: Some("Pablo".into()),
                notes: Some("Fundo de troco inicial".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(shift.status, "OPEN");
        assert_eq!(shift.initial_balance, 150.0);
        assert_eq!(shift.opened_by, "Pablo");
        assert_eq!(events, vec![Event::CashUpdated]);

        // Não pode abrir outro com turno já aberto
        assert!(open_shift(&db, OpenShiftInput::default()).await.is_err());

        // Suprimento e Sangria
        let (tx_sup, _) = add_transaction(
            &db,
            TransactionInput {
                tx_type: "SUPPLY".into(),
                amount: 50.0,
                reason: "Troco extra".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(tx_sup.r#type, "SUPPLY");
        assert_eq!(tx_sup.amount, 50.0);

        let (tx_with, _) = add_transaction(
            &db,
            TransactionInput {
                tx_type: "WITHDRAWAL".into(),
                amount: 30.0,
                reason: "Gelo".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(tx_with.r#type, "WITHDRAWAL");
        assert_eq!(tx_with.amount, 30.0);

        // Estado atual do caixa
        let cur = current_shift(&db).await.unwrap();
        assert!(cur.is_open);
        let summary = cur.summary.unwrap();
        assert_eq!(summary.initial_balance, 150.0);
        assert_eq!(summary.total_supplies, 50.0);
        assert_eq!(summary.total_withdrawals, 30.0);
        // Esperado = 150 + 50 - 30 = 170
        assert_eq!(summary.expected_cash_in_drawer, 170.0);

        // Fechamento cego: contado 160 (falta 10)
        let (closed, report, events) = close_shift(
            &db,
            CloseShiftInput {
                final_cash_count: 160.0,
                closed_by: Some("Pablo".into()),
                notes: Some("Faltou 10 reais".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(closed.status, "CLOSED");
        assert_eq!(closed.final_balance, Some(160.0));
        assert_eq!(closed.expected_balance, Some(170.0));
        assert_eq!(closed.difference, Some(-10.0));
        assert_eq!(report.expected_cash, 170.0);
        assert_eq!(report.counted_cash, 160.0);
        assert_eq!(report.difference, -10.0);
        assert_eq!(events, vec![Event::CashUpdated]);

        // Caixa fechado
        let after_close = current_shift(&db).await.unwrap();
        assert!(!after_close.is_open);

        // Reabertura
        let (reopened, _) = reopen_last_shift(&db).await.unwrap();
        assert_eq!(reopened.status, "OPEN");
        assert_eq!(reopened.final_balance, None);
        assert_eq!(reopened.difference, None);

        let cur_reopened = current_shift(&db).await.unwrap();
        assert!(cur_reopened.is_open);

        // Histórico
        let hist = shift_history(&db, 10).await.unwrap();
        assert_eq!(hist.len(), 1);
    }
}
