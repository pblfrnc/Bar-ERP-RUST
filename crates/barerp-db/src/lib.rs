//! Acesso ao `bar.db` do BarERP.
//!
//! O banco é o mesmo criado pelo Prisma na versão Node (datas em milissegundos, booleanos 0/1, ids cuid),
//! então a instalação do cliente abre aqui sem conversão. Novas colunas entram por [`migrate`].

pub mod audit;
pub mod caixa;
pub mod cash;
pub mod catalog;
pub mod customers;
pub mod dashboard;
pub mod datetime;
pub mod digital_menu;
pub mod events;
pub mod importacao;
pub mod kds;
pub mod legado;
pub mod migrate;
pub mod modifiers;
pub mod notas;
pub mod order_history;
pub mod orders;
pub mod printer;
pub mod products;
pub mod promotions;
pub mod retaguarda;
pub mod staff;
pub mod suppliers;
pub mod table_admin;
pub mod tables;
pub mod waiters;

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, SqlitePool, Transaction};
use tokio::sync::{Mutex, OwnedMutexGuard};

pub use datetime::DbTime;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("erro no banco de dados: {0}")]
    Sql(#[from] sqlx::Error),
    /// Registro não existe (HTTP 404 no servidor)
    #[error("{0}")]
    NotFound(String),
    /// Regra de negócio recusou a operação (HTTP 400); a mensagem vai direto para a tela
    #[error("{0}")]
    Rule(String),
    /// Estado mudou por outra pessoa/aparelho (HTTP 409): atualizar a tela e tentar de novo
    #[error("{0}")]
    Conflict(String),
    /// Bloqueado por configuração/estado do sistema, ex.: caixa fechado (HTTP 403)
    #[error("{0}")]
    Forbidden(String),
}

impl DbError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn rule(msg: impl Into<String>) -> Self {
        Self::Rule(msg.into())
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    /// Uma gravação por vez: o caixa e os celulares lançando na mesma comanda não passam juntos pelas
    /// conferências de saldo/estoque (o Node fazia isso com uma fila por comanda só nos pagamentos)
    writer: Arc<Mutex<()>>,
}

/// Transação de gravação com a vez garantida. Sem `commit` tudo é desfeito ao sair do escopo.
pub struct WriteTx {
    tx: Transaction<'static, Sqlite>,
    _turn: OwnedMutexGuard<()>,
}

impl WriteTx {
    pub async fn commit(self) -> DbResult<()> {
        Ok(self.tx.commit().await?)
    }
}

impl Deref for WriteTx {
    type Target = Transaction<'static, Sqlite>;
    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}

impl DerefMut for WriteTx {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tx
    }
}

impl Db {
    /// Abre (ou cria) o banco no caminho indicado e aplica as migrações.
    pub async fn open(path: &Path) -> DbResult<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            // WAL deixa o caixa, os celulares e o backup lerem enquanto outro grava
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(10));
        Self::connect(options).await
    }

    /// Banco em memória, para testes.
    pub async fn open_in_memory() -> DbResult<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        Self::connect(options).await
    }

    async fn connect(options: SqliteConnectOptions) -> DbResult<Self> {
        let pool = SqlitePoolOptions::new()
            // Em memória cada conexão seria um banco diferente
            .max_connections(if options.get_filename().as_os_str() == ":memory:" { 1 } else { 8 })
            .connect_with(options)
            .await?;
        let db = Self { pool, writer: Arc::new(Mutex::new(())) };
        migrate::run(&db.pool).await?;
        Ok(db)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Começa uma gravação (espera a vez se outra estiver em andamento).
    pub async fn begin_write(&self) -> DbResult<WriteTx> {
        let turn = self.writer.clone().lock_owned().await;
        let tx = self.pool.begin().await?;
        Ok(WriteTx { tx, _turn: turn })
    }
}
