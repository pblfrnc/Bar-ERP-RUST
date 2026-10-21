//! Emissão fiscal do BarERP pela Focus NFe. Nada aqui acessa o banco: quem chama passa os dados
//! (configuração fiscal, itens da venda) e grava o resultado.
//!
//! Tradução de `server/src/routes/fiscal.ts`, `server/src/services/nfeService.ts`,
//! `server/src/services/nfeRecebidasScheduler.ts` e `client/src/services/nfce.ts`.

pub mod chave;
pub mod conversao;
pub mod danfe;
pub mod danfe_nfce;
pub mod error;
pub mod focus;
pub mod num;
pub mod payload;
pub mod publico;
pub mod xml;

pub use chave::{modelo_da_nota, parse_chave_acesso, ChaveAcesso};
pub use danfe::{ajustar_danfe_nfce_html, AjustesDanfe};
pub use danfe_nfce::{ler_danfe_nfce, DanfeNfce, ItemDanfe, PagamentoDanfe};
pub use conversao::{detectar_conversao, ModoConversao, SugestaoConversao};
pub use error::FiscalError;
/// Método HTTP de [`FocusClient::requisitar`].
pub use reqwest::Method;
pub use focus::{
    Ambiente, Arquivo, Emissao, FocusClient, ListaRecebidas, NotaAutorizada, NotaRecebidaFocus, RespostaFocus,
    StatusSefaz, XmlRecebida,
};
pub use payload::{
    codigo_forma_pagamento, montar_nfce, montar_nfe, ratear_desconto, referencia_nfce, referencia_nfe, Destinatario,
    Emitente, FocusPayload, ItemNota, Modelo, NotaMontada, Venda,
};
pub use xml::{ler_nfe_xml, parece_xml_nfe, valor_nf, EmitenteXml, ItemXml, NfeXml};
