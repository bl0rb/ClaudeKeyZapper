//! `GET /office/bootstrap`: token → identity → profile → credential → sparse response. Every outcome maps to a
//! fixed status and a one-word error body; details stay in the structured log.

use crate::auth::JwtVerifier;
use crate::cors::{user_agent_header, CorsPolicy};
use crate::logging::redact;
use axum::extract::State;
use axum::http::header::{HeaderMap, HeaderValue, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, PRAGMA, RETRY_AFTER};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use keyzapper_core::models::McpServer;
use keyzapper_core::office::{self, BootstrapResponse, CredentialError, OfficeCredentialProvider, OfficeDenied, OfficeHost, OfficeProfileResolver, UnassignedPolicy};
use std::sync::Arc;
use std::time::Instant;

pub struct ServerState {
    pub verifier: JwtVerifier,
    pub resolver: Box<dyn OfficeProfileResolver>,
    pub provider: Box<dyn OfficeCredentialProvider>,
    pub cors: CorsPolicy,
    pub mcp_servers: Vec<McpServer>,
    pub unassigned_policy: UnassignedPolicy,
    pub tenant_id: String,
    pub log_salt: String,
}

pub fn router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/office/bootstrap", get(bootstrap).options(preflight))
        .route("/health", get(health))
        .with_state(state)
}

async fn health() -> Response {
    json(StatusCode::OK, r#"{"status":"ok"}"#.to_string())
}

async fn preflight(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    state.cors.preflight(&headers)
}

/// Fixed error codes the client sees; nothing else leaves the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Defaults,
    Unauthorized,
    Forbidden,
    Internal,
    ProviderUnavailable,
}

impl Outcome {
    fn status(self) -> StatusCode {
        match self {
            Outcome::Ok | Outcome::Defaults => StatusCode::OK,
            Outcome::Unauthorized => StatusCode::UNAUTHORIZED,
            Outcome::Forbidden => StatusCode::FORBIDDEN,
            Outcome::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Outcome::ProviderUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Defaults => "defaults",
            Outcome::Unauthorized => "unauthorized",
            Outcome::Forbidden => "forbidden",
            Outcome::Internal => "internal",
            Outcome::ProviderUnavailable => "credential_provider_unavailable",
        }
    }
}

fn json(status: StatusCode, body: String) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// Per-request bookkeeping for the single structured log line.
struct Trace {
    request_id: String,
    started: Instant,
    host: OfficeHost,
    user: Option<String>,
    profile_id: Option<String>,
    detail: Option<String>,
}

impl Trace {
    fn finish(self, outcome: Outcome, provider: &str, tenant: &str) {
        let latency_ms = self.started.elapsed().as_millis() as u64;
        let status = outcome.status().as_u16();
        let user = self.user.as_deref().unwrap_or("-");
        let profile = self.profile_id.as_deref().unwrap_or("-");
        let detail = self.detail.as_deref().map(redact).unwrap_or_default();
        if status < 400 {
            tracing::info!(request_id = %self.request_id, user, tenant, host = %self.host, profile, status, outcome = outcome.code(), latency_ms, provider, "bootstrap");
        } else {
            tracing::warn!(request_id = %self.request_id, user, tenant, host = %self.host, profile, status, outcome = outcome.code(), latency_ms, provider, detail, "bootstrap");
        }
    }
}

async fn bootstrap(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let mut trace = Trace {
        request_id: uuid::Uuid::new_v4().simple().to_string()[..16].to_string(),
        started: Instant::now(),
        host: OfficeHost::from_user_agent(headers.get(user_agent_header()).and_then(|v| v.to_str().ok())),
        user: None,
        profile_id: None,
        detail: None,
    };
    let (outcome, body) = handle(&state, &headers, &mut trace).await;
    let mut response = json(outcome.status(), body);
    if outcome == Outcome::ProviderUnavailable {
        response.headers_mut().insert(RETRY_AFTER, HeaderValue::from_static("10"));
    }
    state.cors.apply(&headers, response.headers_mut());
    trace.finish(outcome, state.provider.kind(), &state.tenant_id);
    response
}

fn error_body(outcome: Outcome) -> String {
    format!(r#"{{"error":"{}"}}"#, outcome.code())
}

async fn handle(state: &Arc<ServerState>, headers: &HeaderMap, trace: &mut Trace) -> (Outcome, String) {
    let token = match JwtVerifier::bearer(headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok())) {
        Ok(token) => token.to_string(),
        Err(other) => {
            trace.detail = Some(other.to_string());
            return (Outcome::Unauthorized, error_body(Outcome::Unauthorized));
        }
    };
    let verifying = Arc::clone(state);
    let verified = tokio::task::spawn_blocking(move || verifying.verifier.verify(&token)).await;
    let identity = match verified {
        Ok(Ok(identity)) => identity,
        Ok(Err(e)) => {
            trace.detail = Some(e.to_string());
            return (Outcome::Unauthorized, error_body(Outcome::Unauthorized));
        }
        Err(_) => {
            trace.detail = Some("verification task failed".into());
            return (Outcome::Internal, error_body(Outcome::Internal));
        }
    };
    trace.user = Some(office::pseudonymous_user_id(&identity.oid, &state.log_salt));
    let resolved = match state.resolver.resolve(&identity, &trace.host) {
        Ok(resolved) => resolved,
        Err(OfficeDenied::NotAssigned) if state.unassigned_policy == UnassignedPolicy::Defaults => {
            return (Outcome::Defaults, serde_json::to_string(&BootstrapResponse::empty()).unwrap_or_else(|_| "{}".into()));
        }
        Err(denied) => {
            trace.detail = Some(denied.to_string());
            return (Outcome::Forbidden, error_body(Outcome::Forbidden));
        }
    };
    trace.profile_id = Some(resolved.profile.id.clone());
    let ttl = resolved.settings.ttl_seconds();
    let vending = Arc::clone(state);
    let (profile, identity_for_key) = (resolved.profile.clone(), identity.clone());
    let credential = tokio::task::spawn_blocking(move || vending.provider.credential_for(&profile, &identity_for_key, ttl)).await;
    let credential = match credential {
        Ok(Ok(credential)) => credential,
        Ok(Err(CredentialError::Unavailable(m))) => {
            trace.detail = Some(m);
            return (Outcome::ProviderUnavailable, error_body(Outcome::ProviderUnavailable));
        }
        Ok(Err(CredentialError::NotConfigured(m))) => {
            trace.detail = Some(m);
            return (Outcome::Internal, error_body(Outcome::Internal));
        }
        Err(_) => {
            trace.detail = Some("credential task failed".into());
            return (Outcome::Internal, error_body(Outcome::Internal));
        }
    };
    let response = office::build_response(&resolved, &credential, &state.mcp_servers, keyzapper_core::oidc::unix_now());
    match serde_json::to_string(&response) {
        Ok(body) => (Outcome::Ok, body),
        Err(_) => {
            trace.detail = Some("response serialisation failed".into());
            (Outcome::Internal, error_body(Outcome::Internal))
        }
    }
}
