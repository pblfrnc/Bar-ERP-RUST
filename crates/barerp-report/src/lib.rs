//! PDFs A4 do BarERP gerados direto em Rust (sem navegador/webview).
//!
//! - [`stock_sheet`]: folha de balanço de estoque e revisão de preços (de `client/src/utils/stockSheet.ts`).
//! - [`table_qr`]: QR Codes do cardápio digital por mesa (de `client/src/utils/tableQrSheet.ts`).
//!
//! Usa as fontes padrão do PDF (Helvetica, Courier) em WinAnsiEncoding: cobre todos os acentos
//! do português sem embutir arquivo de fonte.

mod metrics;
mod pdf;
pub mod stock_sheet;
pub mod table_qr;
mod text;

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("Não foi possível gerar o QR Code de {url}: {message}")]
    Qr { url: String, message: String },
}
