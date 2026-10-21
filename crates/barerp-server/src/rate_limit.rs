//! Limite de requisições por IP em `/api`, como o `express-rate-limit` do Node: 500 a cada 15 minutos
//! (janela fixa por IP). As fotos dos produtos não contam: cada miniatura da lista seria uma requisição.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

const WINDOW: Duration = Duration::from_secs(15 * 60);
const MAX_REQUESTS: u32 = 500;

#[derive(Clone, Default)]
pub struct RateLimiter(Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>);

impl RateLimiter {
    /// Conta mais uma requisição; `false` quando o IP passou do limite na janela atual.
    fn hit(&self, ip: IpAddr, now: Instant) -> bool {
        let mut map = self.0.lock().unwrap();
        map.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        let entry = map.entry(ip).or_insert((now, 0));
        entry.1 += 1;
        entry.1 <= MAX_REQUESTS
    }
}

fn is_product_image(method: &Method, path: &str) -> bool {
    method == Method::GET
        && path
            .strip_prefix("/api/products/")
            .and_then(|rest| rest.strip_suffix("/image"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

pub async fn middleware(State(limiter): State<RateLimiter>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if path.starts_with("/api") && !is_product_image(req.method(), path) {
        let ip = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip().to_canonical())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        if !limiter.hit(ip, Instant::now()) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "Muitas requisições deste IP, tente novamente mais tarde." })),
            )
                .into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_ip_and_window() {
        let limiter = RateLimiter::default();
        let ip: IpAddr = "192.168.0.10".parse().unwrap();
        let other: IpAddr = "192.168.0.11".parse().unwrap();
        let start = Instant::now();
        for _ in 0..MAX_REQUESTS {
            assert!(limiter.hit(ip, start));
        }
        assert!(!limiter.hit(ip, start));
        assert!(limiter.hit(other, start));
        assert!(limiter.hit(ip, start + WINDOW));
    }

    #[test]
    fn product_images_are_free() {
        assert!(is_product_image(&Method::GET, "/api/products/abc/image"));
        assert!(!is_product_image(&Method::PUT, "/api/products/abc/image"));
        assert!(!is_product_image(&Method::GET, "/api/products//image"));
        assert!(!is_product_image(&Method::GET, "/api/products/a/b/image"));
    }
}
