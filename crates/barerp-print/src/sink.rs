//! Para onde vão os bytes: impressora de rede, spooler do Windows ou memória (testes e prévia).

use std::io::Write;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum PrintError {
    #[error("Impressora {addr} não encontrada na rede: {source}")]
    Connect {
        addr: String,
        source: std::io::Error,
    },
    #[error("Falha ao enviar para a impressora {addr}: {source}")]
    Send {
        addr: String,
        source: std::io::Error,
    },
    #[error("Impressora do Windows \"{printer}\": {message}")]
    Spooler { printer: String, message: String },
}

/// Destino de um trabalho de impressão já em bytes ESC/POS.
pub trait PrinterSink {
    /// `job_name` aparece na fila de impressão (spooler); destinos sem fila ignoram.
    fn send(&mut self, job_name: &str, data: &[u8]) -> Result<(), PrintError>;
}

/// Acumula os bytes — para testes e para salvar o trabalho em arquivo.
impl PrinterSink for Vec<u8> {
    fn send(&mut self, _job_name: &str, data: &[u8]) -> Result<(), PrintError> {
        self.extend_from_slice(data);
        Ok(())
    }
}

/// Impressora térmica de rede na porta RAW (JetDirect, 9100 por padrão).
#[derive(Debug, Clone)]
pub struct TcpPrinter {
    /// `192.168.0.50` ou `192.168.0.50:9100`
    pub addr: String,
    pub timeout: Duration,
}

impl TcpPrinter {
    pub const DEFAULT_PORT: u16 = 9100;

    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            timeout: Duration::from_secs(5),
        }
    }

    fn target(&self) -> String {
        let addr = self.addr.trim();
        // IPv6 sem porta vem sem colchetes; host:porta tem exatamente um ':'
        if addr.starts_with('[') || addr.matches(':').count() == 1 {
            addr.to_string()
        } else if addr.contains(':') {
            format!("[{addr}]:{}", Self::DEFAULT_PORT)
        } else {
            format!("{addr}:{}", Self::DEFAULT_PORT)
        }
    }
}

impl PrinterSink for TcpPrinter {
    fn send(&mut self, _job_name: &str, data: &[u8]) -> Result<(), PrintError> {
        let target = self.target();
        let connect_err = |source| PrintError::Connect {
            addr: target.clone(),
            source,
        };
        let addrs: Vec<_> = target.to_socket_addrs().map_err(connect_err)?.collect();
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "endereço sem IP");
        let mut stream = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, self.timeout) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = e,
            }
        }
        let mut stream = stream.ok_or_else(|| connect_err(last))?;
        let send_err = |source| PrintError::Send {
            addr: target.clone(),
            source,
        };
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(send_err)?;
        stream.write_all(data).map_err(send_err)?;
        stream.flush().map_err(send_err)?;
        // Fecha a escrita para a impressora saber que o trabalho acabou
        let _ = stream.shutdown(Shutdown::Write);
        Ok(())
    }
}

/// Envia para a impressora configurada: rede se tiver IP/porta (`.`, `:`), ou spooler Windows pelo nome.
pub fn print_bytes(target: &str, job_name: &str, data: &[u8]) -> Result<(), PrintError> {
    let t = target.trim();
    if t.contains('.') || t.contains(':') {
        let mut p = TcpPrinter::new(t);
        p.send(job_name, data)
    } else {
        #[cfg(windows)]
        {
            let mut p = crate::WindowsPrinter::new(t);
            p.send(job_name, data)
        }
        #[cfg(not(windows))]
        {
            Err(PrintError::Spooler {
                printer: t.to_string(),
                message: "Spooler do Windows só está disponível no Windows. Em desenvolvimento use endereço de rede (IP:9100).".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn porta_padrao_9100() {
        assert_eq!(
            TcpPrinter::new("192.168.0.50").target(),
            "192.168.0.50:9100"
        );
        assert_eq!(
            TcpPrinter::new("impressora.local:9101").target(),
            "impressora.local:9101"
        );
        assert_eq!(TcpPrinter::new("fe80::1").target(), "[fe80::1]:9100");
    }

    #[test]
    fn envia_pela_rede() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            buf
        });
        TcpPrinter::new(format!("127.0.0.1:{port}"))
            .send("teste", b"\x1b@ola\n")
            .unwrap();
        assert_eq!(server.join().unwrap(), b"\x1b@ola\n");
    }

    #[test]
    fn erro_quando_ninguem_escuta() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut p = TcpPrinter::new(format!("127.0.0.1:{port}"));
        p.timeout = Duration::from_millis(300);
        assert!(matches!(p.send("x", b"x"), Err(PrintError::Connect { .. })));
    }
}
