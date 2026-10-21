//! `/api/health` (os celulares testam a conexão por aqui) e `/api/network/devices`.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/network/devices", get(devices))
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "time": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "uptime": state.started.elapsed().as_secs_f64(),
        "ips": local_ips(),
        "port": state.port,
    }))
}

async fn devices(State(state): State<AppState>) -> Json<Value> {
    Json(serde_json::to_value(state.devices.list()).unwrap_or_default())
}

/// Placas que não são o Wi-Fi/cabo do bar (máquinas virtuais, VPN).
fn is_virtual(name: &str) -> bool {
    let name = name.to_lowercase();
    ["virtual", "vmware", "vethernet", "wsl", "tailscale", "hamachi"].iter().any(|v| name.contains(v))
}

/// IPv4 da rede local, com os 192.168.x.x primeiro (o celular quase sempre está nessa faixa).
pub fn local_ips() -> Vec<String> {
    let mut ips = Vec::new();
    for iface in if_addrs::get_if_addrs().unwrap_or_default() {
        if is_virtual(&iface.name) || iface.is_loopback() {
            continue;
        }
        if let std::net::IpAddr::V4(ip) = iface.ip() {
            let text = ip.to_string();
            if text.starts_with("192.168.") {
                ips.insert(0, text);
            } else {
                ips.push(text);
            }
        }
    }
    ips
}
