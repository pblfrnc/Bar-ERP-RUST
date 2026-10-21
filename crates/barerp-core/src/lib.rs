//! Regras de negócio puras do BarERP: nada aqui acessa banco, rede ou impressora.
//!
//! Cada módulo é a tradução direta de um arquivo do servidor Node, com o mesmo comportamento,
//! para que o `bar.db` e os APKs atuais continuem funcionando depois da troca.

pub mod fiscal_doc;
pub mod ids;
pub mod money;
pub mod pricing;
pub mod time;
pub mod vault;
