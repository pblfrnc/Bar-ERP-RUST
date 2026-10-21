//! Impressora instalada no Windows (USB/compartilhada), recebendo os bytes crus pelo spooler (datatype RAW).

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{GetLastError, HANDLE};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, EndDocPrinter, EndPagePrinter, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW, StartDocPrinterW,
    StartPagePrinter, WritePrinter, DOC_INFO_1W, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
    PRINTER_INFO_4W,
};

use crate::sink::{PrintError, PrinterSink};

/// Impressora pelo nome que aparece em "Impressoras e scanners".
#[derive(Debug, Clone)]
pub struct WindowsPrinter {
    pub name: String,
}

impl WindowsPrinter {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    fn err(&self, step: &str) -> PrintError {
        let code = unsafe { GetLastError() };
        PrintError::Spooler {
            printer: self.name.clone(),
            message: format!("{step} falhou ({:?})", code),
        }
    }
}

/// Fecha a impressora ao sair, mesmo com erro no meio.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = ClosePrinter(self.0);
        }
    }
}

impl PrinterSink for WindowsPrinter {
    fn send(&mut self, job_name: &str, data: &[u8]) -> Result<(), PrintError> {
        let name = HSTRING::from(self.name.as_str());
        let mut raw = HANDLE::default();
        unsafe { OpenPrinterW(PCWSTR(name.as_ptr()), &mut raw, None) }.map_err(|e| {
            PrintError::Spooler {
                printer: self.name.clone(),
                message: format!("não foi possível abrir: {e}"),
            }
        })?;
        let handle = Handle(raw);

        let mut doc_name: Vec<u16> = job_name.encode_utf16().chain(Some(0)).collect();
        let mut datatype: Vec<u16> = "RAW".encode_utf16().chain(Some(0)).collect();
        let info = DOC_INFO_1W {
            pDocName: PWSTR(doc_name.as_mut_ptr()),
            pOutputFile: PWSTR::null(),
            pDatatype: PWSTR(datatype.as_mut_ptr()),
        };
        unsafe {
            if StartDocPrinterW(handle.0, 1, &info) == 0 {
                return Err(self.err("StartDocPrinter"));
            }
            if !StartPagePrinter(handle.0).as_bool() {
                let _ = EndDocPrinter(handle.0);
                return Err(self.err("StartPagePrinter"));
            }
            let mut written = 0u32;
            let ok = WritePrinter(
                handle.0,
                data.as_ptr().cast(),
                data.len() as u32,
                &mut written,
            )
            .as_bool();
            let _ = EndPagePrinter(handle.0);
            let _ = EndDocPrinter(handle.0);
            if !ok {
                return Err(self.err("WritePrinter"));
            }
            if written as usize != data.len() {
                return Err(PrintError::Spooler {
                    printer: self.name.clone(),
                    message: format!("enviados {written} de {} bytes", data.len()),
                });
            }
        }
        Ok(())
    }
}

/// Nomes das impressoras locais e conectadas, para a tela de configuração.
pub fn installed_printers() -> Vec<String> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut needed = 0u32;
    let mut count = 0u32;
    unsafe {
        let _ = EnumPrintersW(flags, PCWSTR::null(), 4, None, &mut needed, &mut count);
        if needed == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; needed as usize];
        if EnumPrintersW(
            flags,
            PCWSTR::null(),
            4,
            Some(&mut buf),
            &mut needed,
            &mut count,
        )
        .is_err()
        {
            return Vec::new();
        }
        let infos =
            std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, count as usize);
        infos
            .iter()
            .filter_map(|i| i.pPrinterName.to_string().ok())
            .collect()
    }
}

/// Impressora padrão do Windows ("Padrão do Sistema Operacional" no original), se houver.
pub fn default_printer() -> Option<String> {
    let mut len = 0u32;
    unsafe {
        let _ = GetDefaultPrinterW(PWSTR::null(), &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if !GetDefaultPrinterW(PWSTR(buf.as_mut_ptr()), &mut len).as_bool() {
            return None;
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16(&buf[..end]).ok().filter(|n| !n.is_empty())
    }
}
