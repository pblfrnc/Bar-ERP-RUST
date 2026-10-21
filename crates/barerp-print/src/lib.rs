//! Impressão térmica ESC/POS do BarERP.
//!
//! - [`EscPos`]: builder que gera os bytes da impressora e uma prévia em texto puro.
//! - [`receipt`] e [`kitchen`]: layouts portados de `ThermalReceipt.tsx` e `KitchenTicketReceipt.tsx`.
//! - [`PrinterSink`]: envio para impressora de rede (TCP 9100), spooler do Windows (RAW) ou CUPS (macOS/Linux).
//! - [`installed_printers`] / [`default_printer`] / [`SystemPrinter`]: impressoras instaladas no sistema, em
//!   qualquer um dos dois.

mod codepage;
#[cfg(unix)]
mod cups;
mod codepage_tables;
mod escpos;
pub mod format;
pub mod kitchen;
pub mod receipt;
mod sink;
#[cfg(windows)]
mod windows_spooler;

pub use codepage::Codepage;
pub use escpos::{Align, DrawerPin, EscPos, PaperWidth, QrEcc};
pub use sink::{print_bytes, PrintError, PrinterSink, TcpPrinter};
#[cfg(windows)]
pub use windows_spooler::{default_printer, installed_printers, WindowsPrinter};
#[cfg(unix)]
pub use cups::{default_printer, installed_printers, CupsPrinter};

/// Impressora instalada no sistema: spooler RAW no Windows, CUPS no macOS/Linux.
#[cfg(windows)]
pub type SystemPrinter = WindowsPrinter;
#[cfg(unix)]
pub type SystemPrinter = CupsPrinter;
