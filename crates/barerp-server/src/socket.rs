//! Socket.IO: aparelhos conectados (`devices_updated`) e estações de impressão dos recibos.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{ConnectInfo, Query};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use socketioxide::extract::SocketRef;
use socketioxide::layer::SocketIoLayer;
use socketioxide::{SocketIo, TransportType};
use tokio::sync::{mpsc, oneshot};

use crate::events::broadcast;
use crate::AppState;

/// Sala onde os caixas Electron se registram para imprimir os recibos pedidos pelo celular.
pub const PRINT_STATIONS_ROOM: &str = "print-stations";

/// Mesmo ajuste do Node para aguentar oscilação do Wi-Fi e economia de bateria do celular.
pub fn layer() -> (SocketIoLayer, SocketIo) {
    SocketIo::builder()
        .ping_interval(Duration::from_secs(5))
        .ping_timeout(Duration::from_secs(10))
        .transports([TransportType::Websocket, TransportType::Polling])
        .build_layer()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedDevice {
    pub socket_id: String,
    pub ip: String,
    pub user_agent: String,
    pub connected_at: String,
    /// CAIXA_PC, GARCOM_MOBILE ou COZINHA_KDS
    pub client_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiter_name: Option<String>,
}

#[derive(Default)]
struct DevicesInner {
    /// Ordem de conexão preservada para a lista sair igual à do `Map` do Node
    list: Vec<ConnectedDevice>,
    /// Estações de impressão na ordem em que se registraram
    stations: Vec<String>,
}

#[derive(Clone, Default)]
pub struct Devices(Arc<Mutex<DevicesInner>>);

impl Devices {
    pub fn list(&self) -> Vec<ConnectedDevice> {
        self.0.lock().unwrap().list.clone()
    }

    fn add(&self, device: ConnectedDevice) {
        self.0.lock().unwrap().list.push(device);
    }

    fn remove(&self, sid: &str) {
        let mut inner = self.0.lock().unwrap();
        inner.list.retain(|d| d.socket_id != sid);
        inner.stations.retain(|s| s != sid);
    }

    fn add_station(&self, sid: String) {
        let mut inner = self.0.lock().unwrap();
        if !inner.stations.contains(&sid) {
            inner.stations.push(sid);
        }
    }

    /// Estação registrada há mais tempo (o Node mandava só para ela, para não imprimir em duplicidade).
    pub fn first_station(&self) -> Option<String> {
        self.0.lock().unwrap().stations.first().cloned()
    }
}

/// Recibo pedido pelo celular para a tela nativa imprimir. Responda em `reply` (`Err` com a mensagem
/// que o garçom vai ver).
pub struct PrintRequest {
    /// Comanda no formato do Prisma, com `table`, `items` e `payments`
    pub order: Value,
    pub requested_by: Option<String>,
    pub reply: oneshot::Sender<Result<(), String>>,
}

#[derive(Clone, Default)]
pub struct LocalPrinter(Arc<Mutex<Option<mpsc::Sender<PrintRequest>>>>);

impl LocalPrinter {
    pub fn set(&self, tx: mpsc::Sender<PrintRequest>) {
        *self.0.lock().unwrap() = Some(tx);
    }

    pub fn get(&self) -> Option<mpsc::Sender<PrintRequest>> {
        self.0.lock().unwrap().as_ref().filter(|tx| !tx.is_closed()).cloned()
    }
}

fn iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn register(io: &SocketIo, state: AppState) {
    io.ns("/", move |socket: SocketRef| {
        let parts = socket.req_parts();
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip().to_canonical().to_string())
            .unwrap_or_default();
        let user_agent = parts
            .headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("Desconhecido")
            .to_string();
        let query = Query::<HashMap<String, String>>::try_from_uri(&parts.uri).map(|q| q.0).unwrap_or_default();
        let client_type = query.get("clientType").filter(|c| !c.is_empty()).cloned().unwrap_or_else(|| "Desconhecido".into());
        let waiter_name = query.get("waiterName").filter(|w| !w.is_empty()).cloned();

        tracing::info!("dispositivo conectado ({client_type}): {} (IP: {ip})", socket.id);
        state.devices.add(ConnectedDevice {
            socket_id: socket.id.to_string(),
            ip,
            user_agent,
            connected_at: iso(Utc::now()),
            client_type,
            waiter_name,
        });
        broadcast(&state, "devices_updated", serde_json::to_value(state.devices.list()).unwrap_or_default());

        let devices = state.devices.clone();
        socket.on("print-station:register", move |socket: SocketRef| {
            if let Err(e) = socket.join(PRINT_STATIONS_ROOM) {
                tracing::warn!("estação de impressão {}: {e}", socket.id);
                return;
            }
            devices.add_station(socket.id.to_string());
            tracing::info!("estação de impressão registrada: {}", socket.id);
        });

        let state = state.clone();
        socket.on_disconnect(move |socket: SocketRef| {
            state.devices.remove(&socket.id.to_string());
            tracing::info!("dispositivo desconectado: {}", socket.id);
            broadcast(&state, "devices_updated", serde_json::to_value(state.devices.list()).unwrap_or_default());
        });
    });
}
