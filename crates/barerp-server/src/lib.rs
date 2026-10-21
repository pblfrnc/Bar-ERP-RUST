//! Servidor embutido do BarERP: a mesma API REST e o mesmo Socket.IO do `server/` em Node, na porta 3001.
//!
//! Os APKs de Garçom e Cozinha não mudam, então as respostas seguem o formato do Prisma (camelCase, datas em
//! ISO, `activeOrder` com itens, produto e pagamentos) e os eventos têm os mesmos nomes e payloads.

mod error;
mod events;
mod fiscal;
mod http;
mod prisma;
mod rate_limit;
mod routes;
mod socket;
#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Router;
use barerp_db::events::Event;
use barerp_db::Db;
use socketioxide::SocketIo;
use tokio::net::TcpListener;

pub use error::ApiError;
pub use fiscal::{CampoMultipart, Corpo, RespostaFiscal};
pub use events::ServerEvent;
pub use routes::system::SystemServices;
pub use socket::{ConnectedDevice, PrintRequest};

/// Porta do Node, que os celulares já têm salva.
pub const DEFAULT_PORT: u16 = 3001;

#[derive(Clone)]
pub(crate) struct AppState {
    pub db: Db,
    pub io: SocketIo,
    pub devices: socket::Devices,
    pub printer: socket::LocalPrinter,
    pub started: Instant,
    pub port: u16,
    pub pay_requests: routes::orders::PayRequests,
    pub events: tokio::sync::broadcast::Sender<ServerEvent>,
    /// Por que o servidor não subiu (a tela mostra no rodapé)
    pub startup_error: Arc<Mutex<Option<String>>>,
    /// Backup e cardápio digital do app (rotas `/api/system`)
    pub system: routes::system::SharedSystem,
}

/// Servidor montado: o roteador para servir e o canal de eventos para a tela nativa avisar os celulares.
pub struct Server {
    router: Router,
    handle: ServerHandle,
    fiscal: fiscal::FiscalState,
}

/// Usado pela tela nativa: repassa aos celulares o que ela gravou e recebe os recibos pedidos pelo celular.
#[derive(Clone)]
pub struct ServerHandle {
    state: AppState,
    fiscal: fiscal::FiscalState,
}

impl ServerHandle {
    /// Emite no Socket.IO os eventos devolvidos pelas funções do `barerp-db`.
    pub async fn emit(&self, events: &[Event]) {
        events::emit_all(&self.state, events).await;
    }

    /// O que os celulares mudaram (e o que a própria tela emitiu): a tela nativa recarrega o mapa de mesas,
    /// a comanda aberta ou o caixa na hora, sem esperar a atualização periódica.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ServerEvent> {
        self.state.events.subscribe()
    }

    /// Aparelhos conectados agora (tela "Rede" do caixa).
    pub fn devices(&self) -> Vec<ConnectedDevice> {
        self.state.devices.list()
    }

    /// A tela nativa passa a imprimir os recibos pedidos pelo celular (`POST /api/orders/:id/print-receipt`).
    /// Sem isso, o pedido vai para um caixa Electron registrado no Socket.IO, como no Node.
    pub fn set_local_printer(&self, tx: tokio::sync::mpsc::Sender<PrintRequest>) {
        self.state.printer.set(tx);
    }

    /// O app entrega o backup e o publicador do cardápio que ele já roda (com os agendadores), para as rotas
    /// `/api/system` não criarem outros. Só vale a primeira chamada.
    pub fn set_system_services(&self, services: SystemServices) {
        let _ = self.state.system.set(services);
    }

    /// Endereços IPv4 da rede local (QR de conexão dos celulares).
    pub fn local_ips(&self) -> Vec<String> {
        routes::health::local_ips()
    }

    pub fn port(&self) -> u16 {
        self.state.port
    }

    /// Rotas `/api/fiscal` chamadas pela tela nativa sem passar pela rede (`caminho` sem o prefixo).
    pub async fn fiscal(&self, metodo: &str, caminho: &str, corpo: Corpo) -> RespostaFiscal {
        fiscal::chamar(&self.fiscal, metodo, caminho, corpo).await
    }

    /// Mensagem para o caixa quando o servidor não conseguiu subir ou caiu (ex.: porta 3001 ocupada pelo
    /// BarERP antigo). `None` enquanto está tudo certo ou ainda subindo.
    pub fn startup_error(&self) -> Option<String> {
        self.state.startup_error.lock().unwrap().clone()
    }
}

impl ServerHandle {
    fn fail(&self, kind: std::io::ErrorKind, msg: String) -> std::io::Error {
        tracing::error!("{msg}");
        *self.state.startup_error.lock().unwrap() = Some(msg.clone());
        std::io::Error::new(kind, msg)
    }
}

impl Server {
    /// `data_dir` é a pasta do `bar.db` (o fiscal guarda os XMLs nela).
    pub fn new(db: Db, data_dir: PathBuf, port: u16) -> Self {
        let (layer, io) = socket::layer();
        let state = AppState {
            db,
            io: io.clone(),
            devices: socket::Devices::default(),
            printer: socket::LocalPrinter::default(),
            started: Instant::now(),
            port,
            pay_requests: routes::orders::PayRequests::default(),
            events: tokio::sync::broadcast::channel(256).0,
            startup_error: Arc::default(),
            system: Arc::default(),
        };
        socket::register(&io, state.clone());
        let fiscal = fiscal::FiscalState::new(state.db.clone(), data_dir);
        let router = http::router(state.clone(), fiscal.clone(), layer);
        Self { router, handle: ServerHandle { state, fiscal: fiscal.clone() }, fiscal }
    }

    pub fn handle(&self) -> ServerHandle {
        self.handle.clone()
    }

    /// Roteador completo (testes chamam com `oneshot`).
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Escuta em 0.0.0.0 para os celulares do Wi-Fi do bar alcançarem o caixa.
    /// Se a porta estiver ocupada, devolve o erro com a mensagem para o caixa e a guarda em
    /// [`ServerHandle::startup_error`]; o app segue funcionando, só os celulares não conectam.
    pub async fn serve(self) -> std::io::Result<()> {
        let port = self.handle.state.port;
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = match TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::AddrInUse {
                    format!("A porta {port} já está em uso. Feche o BarERP antigo (Electron) para os celulares conectarem.")
                } else {
                    format!("Não foi possível abrir a porta {port} para os celulares: {e}")
                };
                return Err(self.handle.fail(e.kind(), msg));
            }
        };
        // Sincronização das NF-e recebidas na SEFAZ (como o Node: 10 s depois de subir, depois a cada 15 min)
        fiscal::iniciar_agendador(self.fiscal.clone());
        tracing::info!("servidor dos celulares em http://{addr}");
        for ip in routes::health::local_ips() {
            tracing::info!("  celulares/tablets: http://{ip}:{}", self.handle.state.port);
        }
        let handle = self.handle.clone();
        axum::serve(listener, self.router.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .map_err(|e| handle.fail(e.kind(), format!("O servidor dos celulares parou: {e}")))
    }
}
