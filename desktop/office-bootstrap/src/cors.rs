//! CORS for the add-in: the task pane at `https://pivot.claude.ai` fetches the bootstrap URL from the browser, so
//! the response must name that origin explicitly. `*` is never emitted; unknown origins get no CORS headers at all.

use axum::http::header::{HeaderMap, HeaderName, HeaderValue, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_MAX_AGE, ORIGIN, VARY};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use keyzapper_core::office::{DEFAULT_ALLOWED_ORIGIN, USER_AGENT_HEADER};

/// What a preflight may announce: the add-in sends `Authorization` and `X-Claude-User-Agent`; `*` covers further
/// non-credential headers the add-in may add later.
pub const ALLOWED_HEADERS: &str = "Authorization, X-Claude-User-Agent, *";
pub const ALLOWED_METHODS: &str = "GET";
const MAX_AGE_SECONDS: &str = "600";

#[derive(Clone, Debug)]
pub struct CorsPolicy {
    origins: Vec<String>,
}

fn normalize(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_lowercase()
}

impl Default for CorsPolicy {
    fn default() -> Self {
        Self::new(vec![DEFAULT_ALLOWED_ORIGIN.to_string()])
    }
}

impl CorsPolicy {
    /// `*` and empty entries are dropped; an empty list allows no origin.
    pub fn new(origins: Vec<String>) -> Self {
        Self { origins: origins.iter().map(|o| normalize(o)).filter(|o| !o.is_empty() && o != "*").collect() }
    }

    pub fn origins(&self) -> &[String] {
        &self.origins
    }

    /// The `Origin` request header if it is allowed, as sent (for echoing back).
    pub fn allowed_origin(&self, headers: &HeaderMap) -> Option<String> {
        let origin = headers.get(ORIGIN)?.to_str().ok()?;
        let normalized = normalize(origin);
        self.origins.contains(&normalized).then(|| origin.trim().to_string())
    }

    /// Adds the CORS headers for an allowed origin; does nothing for other origins.
    pub fn apply(&self, request_headers: &HeaderMap, response_headers: &mut HeaderMap) {
        let Some(origin) = self.allowed_origin(request_headers) else { return };
        if let Ok(value) = HeaderValue::from_str(&origin) {
            response_headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, value);
        }
        response_headers.insert(VARY, HeaderValue::from_static("Origin"));
        response_headers.insert(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static(ALLOWED_METHODS));
        response_headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static(ALLOWED_HEADERS));
        response_headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static(MAX_AGE_SECONDS));
    }

    /// `OPTIONS` preflight: `204` with the CORS headers for an allowed origin, `403` otherwise.
    pub fn preflight(&self, request_headers: &HeaderMap) -> Response {
        if self.allowed_origin(request_headers).is_none() {
            return StatusCode::FORBIDDEN.into_response();
        }
        let mut response = StatusCode::NO_CONTENT.into_response();
        self.apply(request_headers, response.headers_mut());
        response
    }
}

/// `X-Claude-User-Agent` as a typed header name (lowercase, as HTTP header names are).
pub fn user_agent_header() -> HeaderName {
    HeaderName::from_lowercase(USER_AGENT_HEADER.to_lowercase().as_bytes()).expect("static header name")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(origin: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert(ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        headers
    }

    #[test]
    fn default_allows_only_pivot() {
        let policy = CorsPolicy::default();
        assert_eq!(policy.allowed_origin(&headers(Some("https://pivot.claude.ai"))).as_deref(), Some("https://pivot.claude.ai"));
        assert_eq!(policy.allowed_origin(&headers(Some("HTTPS://PIVOT.CLAUDE.AI/"))).as_deref(), Some("HTTPS://PIVOT.CLAUDE.AI/"));
        assert!(policy.allowed_origin(&headers(Some("https://evil.example"))).is_none());
        assert!(policy.allowed_origin(&headers(Some("https://pivot.claude.ai.evil.example"))).is_none());
        assert!(policy.allowed_origin(&headers(None)).is_none());
    }

    #[test]
    fn wildcard_is_never_accepted() {
        let policy = CorsPolicy::new(vec!["*".into(), "".into(), "https://pivot.claude.ai".into()]);
        assert_eq!(policy.origins(), ["https://pivot.claude.ai"]);
        assert!(CorsPolicy::new(vec!["*".into()]).allowed_origin(&headers(Some("https://x.example"))).is_none());
    }

    #[test]
    fn preflight_answers_allowed_origins_only() {
        let policy = CorsPolicy::default();
        let ok = policy.preflight(&headers(Some("https://pivot.claude.ai")));
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);
        assert_eq!(ok.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), "https://pivot.claude.ai");
        assert_eq!(ok.headers().get(ACCESS_CONTROL_ALLOW_METHODS).unwrap(), "GET");
        assert_eq!(ok.headers().get(ACCESS_CONTROL_ALLOW_HEADERS).unwrap(), ALLOWED_HEADERS);
        let denied = policy.preflight(&headers(Some("https://evil.example")));
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert!(denied.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    }
}
