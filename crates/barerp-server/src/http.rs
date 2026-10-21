//! Montagem das rotas e camadas na mesma ordem do `index.ts` do Node.

use axum::async_trait;
use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderName, HeaderValue};
use axum::{middleware, Router};
use serde_json::{Map, Value};
use socketioxide::layer::SocketIoLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::error::ApiError;
use crate::rate_limit::{self, RateLimiter};
use crate::{fiscal, routes, AppState};

/// Cabeçalhos do `helmet()` com `contentSecurityPolicy` e `crossOriginEmbedderPolicy` desligados.
/// `if_not_present`: a foto do produto troca o `Cross-Origin-Resource-Policy` para `cross-origin`.
const HELMET: [(&str, &str); 9] = [
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    ("origin-agent-cluster", "?1"),
    ("referrer-policy", "no-referrer"),
    ("x-content-type-options", "nosniff"),
    ("x-dns-prefetch-control", "off"),
    ("x-download-options", "noopen"),
    ("x-frame-options", "SAMEORIGIN"),
    ("x-permitted-cross-domain-policies", "none"),
];

pub fn router(state: AppState, fiscal_state: fiscal::FiscalState, socket_layer: SocketIoLayer) -> Router {
    let api = Router::new()
        .merge(routes::health::router())
        .nest("/api/staff", routes::staff::router())
        .nest("/api/tables", routes::tables::router())
        .nest("/api/orders", routes::orders::router())
        .nest("/api/products", routes::products::router())
        .nest("/api/cash", routes::cash::router())
        .nest("/api/kds", routes::kds::router())
        .nest("/api/settings", routes::settings::router())
        .nest("/api/waiters", routes::waiters::router())
        .nest("/api/customers", routes::customers::router())
        .nest("/api/suppliers", routes::suppliers::router())
        .nest("/api/dashboard", routes::reports::dashboard_router())
        .nest("/api/audit-logs", routes::reports::audit_router())
        .nest("/api/system", routes::system::router())
        .nest("/api/import-xml", routes::import_xml::router())
        .with_state(state)
        .nest("/api/fiscal", fiscal::router(fiscal_state));

    let mut app = api
        .layer(middleware::from_fn_with_state(RateLimiter::default(), rate_limit::middleware))
        .layer(CompressionLayer::new());
    for (name, value) in HELMET {
        app = app.layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        ));
    }
    // O Socket.IO fica por fora das rotas, como no Node (preso ao servidor HTTP, não ao Express),
    // e o CORS aberto vale para os dois: o app do celular roda em outra origem (Capacitor)
    app.layer(socket_layer).layer(CorsLayer::permissive())
}

/// Corpo JSON tolerante como o `express.json()`: sem corpo ou sem `Content-Type` vira `{}`, e os campos
/// são lidos com as conversões soltas do JavaScript (`Number(x)`, `x || padrão`).
pub struct Body(pub Map<String, Value>);

#[async_trait]
impl<S: Send + Sync> FromRequest<S> for Body {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|_| ApiError::bad_request("Corpo da requisição inválido"))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(Map::new()));
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => Ok(Self(map)),
            Ok(_) => Ok(Self(Map::new())),
            Err(_) => Err(ApiError::bad_request("JSON inválido no corpo da requisição")),
        }
    }
}

impl Body {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    /// Texto não vazio (`x || undefined`).
    pub fn str(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// `Number(x)` do JavaScript; `None` quando não é número (NaN).
    pub fn num(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(js_number)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).map(js_truthy)
    }

    pub fn array(&self, key: &str) -> Option<&Vec<Value>> {
        self.get(key).and_then(Value::as_array)
    }
}

/// `Number(x)`: números, textos numéricos ("12", " 3.5 ", "" = 0) e booleanos.
pub fn js_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if s.trim().is_empty() => Some(0.0),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Null => Some(0.0),
        _ => None,
    }
}

/// `Boolean(x)`.
pub fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
