//! Impressora instalada no macOS/Linux pelo CUPS: lista (`lpstat -e`), padrão do sistema (`lpstat -d`) e envio
//! dos bytes crus (`lp -d <fila> -o raw`), o equivalente ao spooler RAW do Windows.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::sink::{PrintError, PrinterSink};

/// Fila do CUPS pelo nome que aparece em "Impressoras e Scanners".
#[derive(Debug, Clone)]
pub struct CupsPrinter {
    pub name: String,
}

impl CupsPrinter {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Saída em inglês (os textos do `lpstat` são traduzidos conforme o idioma do sistema).
fn lpstat(args: &[&str]) -> Option<String> {
    let out = Command::new("lpstat").args(args).env("LC_ALL", "C").env("LANG", "C").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `lpstat -e`: uma fila por linha.
pub fn parse_destinations(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// `lpstat -d`: "system default destination: NOME" ou "no system default destination". O macOS traduz a frase
/// mesmo com `LC_ALL=C` ("destino padrão de sistema: NOME"), então vale o que vem depois dos dois-pontos; sem
/// impressora padrão a frase não tem dois-pontos.
pub fn parse_default(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.split_once(':').map(|(_, name)| name.trim().to_string()))
        .filter(|n| !n.is_empty())
}

/// Filas instaladas no CUPS.
pub fn installed_printers() -> Vec<String> {
    lpstat(&["-e"]).map(|t| parse_destinations(&t)).unwrap_or_default()
}

/// Impressora padrão do sistema, se houver.
pub fn default_printer() -> Option<String> {
    lpstat(&["-d"]).and_then(|t| parse_default(&t))
}

impl PrinterSink for CupsPrinter {
    fn send(&mut self, job_name: &str, data: &[u8]) -> Result<(), PrintError> {
        let err = |message: String| PrintError::Spooler { printer: self.name.clone(), message };
        let mut child = Command::new("lp")
            .args(["-d", &self.name, "-o", "raw", "-t", job_name])
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| err(format!("não foi possível chamar o lp: {e}")))?;
        child
            .stdin
            .take()
            .ok_or_else(|| err("sem entrada para o lp".into()))?
            .write_all(data)
            .map_err(|e| err(format!("falha ao enviar os dados: {e}")))?;
        let out = child.wait_with_output().map_err(|e| err(e.to_string()))?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(err(if msg.is_empty() { "o CUPS recusou o trabalho".into() } else { msg }));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lpstat_output() {
        assert_eq!(parse_destinations("POS_80\nEPSON_TM_T20\n\n"), vec!["POS_80", "EPSON_TM_T20"]);
        assert_eq!(parse_default("system default destination: POS_80\n").as_deref(), Some("POS_80"));
        assert_eq!(parse_default("no system default destination\n"), None);
        assert_eq!(parse_default("destino padrão de sistema: Canon_G3010_series\n").as_deref(), Some("Canon_G3010_series"));
        assert_eq!(parse_default("nenhum destino padrão de sistema\n"), None);
    }
}
