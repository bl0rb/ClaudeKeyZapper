//! LiteLLM checks: `GET /v1/models` (models the key may use) and `GET /key/info` (budget of the calling key),
//! plus single `POST /v1/messages` calls for the repository analysis.
//! TLS uses the system trust store (native-tls), so company CAs installed by IT are trusted like in the browser.

use crate::l;
use serde::Serialize;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum CheckResult {
    /// `missing_models` is None when no model is configured, empty when all configured models are available.
    Ok { missing_models: Option<Vec<String>> },
    Unauthorized,
    RateLimited,
    BudgetExceeded,
    Unreachable(String),
    HttpError(u16),
}

impl CheckResult {
    pub fn message(&self) -> String {
        match self {
            CheckResult::Ok { missing_models: None } => l!("Verbindung erfolgreich."),
            CheckResult::Ok { missing_models: Some(m) } if m.is_empty() => l!("Verbindung erfolgreich, alle Modelle sind verfügbar."),
            CheckResult::Ok { missing_models: Some(m) } => {
                l!("Verbindung erfolgreich, aber diese Modelle sind für den Key nicht freigegeben: %@", m.join(", "))
            }
            CheckResult::Unauthorized => l!("Key wird vom Gateway abgelehnt (ungültig oder gesperrt)."),
            CheckResult::RateLimited => l!("Rate-Limit erreicht. Später erneut versuchen."),
            CheckResult::BudgetExceeded => l!("Budget dieses Keys ist ausgeschöpft."),
            CheckResult::Unreachable(reason) => l!("Gateway nicht erreichbar (VPN?): %@", reason),
            CheckResult::HttpError(code) => l!("Unerwartete Antwort vom Gateway (HTTP %@).", code),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, CheckResult::Ok { missing_models } if missing_models.as_ref().is_none_or(|m| m.is_empty()))
    }
}

/// Budget of a LiteLLM key in USD, as reported by `/key/info`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyBudget {
    pub spend: f64,
    /// None when the key has no budget limit.
    pub max_budget: Option<f64>,
    /// As LiteLLM reports it (Python ISO date).
    pub reset_at: Option<String>,
}

impl KeyBudget {
    /// Remaining budget; infinite without a limit.
    pub fn remaining(&self) -> f64 {
        self.max_budget.map(|max| (max - self.spend).max(0.0)).unwrap_or(f64::INFINITY)
    }
}

/// HTTP agent with the system trust store; non-2xx statuses are returned, not raised.
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build())
        .build()
        .into()
}

/// `agent` for KeyZapper's own internet traffic (GitHub, claude.ai): through the proxy IT, the environment or the system
/// settings name, see `proxy`. Gateway and identity-provider calls keep `agent` and stay direct.
pub fn internet_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build())
        .proxy(crate::proxy::internet())
        .build()
        .into()
}

pub fn join_url(endpoint: &str, path: &str) -> String {
    format!("{}/{}", endpoint.trim_end_matches('/'), path)
}

fn get(path: &str, endpoint: &str, key: &str, timeout: Duration) -> Result<String, CheckResult> {
    let response = agent(timeout)
        .get(&join_url(endpoint, path))
        .header("Authorization", &format!("Bearer {key}"))
        .header("x-api-key", key)
        .call();
    let mut response = response.map_err(|e| CheckResult::Unreachable(e.to_string()))?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    match status {
        200 => Ok(body),
        other => Err(failure(other, &body)),
    }
}

fn failure(status: u16, body: &str) -> CheckResult {
    let lower = body.to_lowercase();
    match status {
        401 | 403 => CheckResult::Unauthorized,
        429 | 400 if lower.contains("budget") => CheckResult::BudgetExceeded,
        429 => CheckResult::RateLimited,
        other => CheckResult::HttpError(other),
    }
}

/// Text and token usage of a Messages API reply.
pub struct Completion {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One `POST /v1/messages` call in Anthropic format (as Claude Code talks to LiteLLM), billed to `key`.
pub fn complete(endpoint: &str, key: &str, model: &str, system: &str, prompt: &str, max_tokens: u32, timeout: Duration) -> Result<Completion, String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let response = agent(timeout)
        .post(&join_url(endpoint, "v1/messages"))
        .header("Authorization", &format!("Bearer {key}"))
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .send(body.to_string());
    let mut response = response.map_err(|e| CheckResult::Unreachable(e.to_string()).message())?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if status != 200 {
        return Err(match (failure(status, &text), value["error"]["message"].as_str()) {
            (CheckResult::HttpError(code), Some(message)) => {
                l!("Das Gateway hat die Anfrage abgelehnt (HTTP %@): %@", code, message.chars().take(400).collect::<String>())
            }
            (other, _) => other.message(),
        });
    }
    match value["stop_reason"].as_str() {
        Some("refusal") => return Err(l!("Das Modell hat die Anfrage abgelehnt.")),
        Some("max_tokens") => return Err(l!("Die Antwort des Modells wurde abgeschnitten (max_tokens).")),
        _ => {}
    }
    let text: String = value["content"]
        .as_array()
        .map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect())
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(l!("Das Gateway hat eine leere Antwort geliefert."));
    }
    Ok(Completion {
        text,
        input_tokens: value["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: value["usage"]["output_tokens"].as_u64().unwrap_or(0),
    })
}

/// Model IDs available to the key, sorted.
pub fn models(endpoint: &str, key: &str) -> Result<Vec<String>, CheckResult> {
    let body = get("v1/models", endpoint, key, Duration::from_secs(10))?;
    let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let mut ids: Vec<String> = value["data"]
        .as_array()
        .map(|items| items.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    ids.sort();
    Ok(ids)
}

/// LiteLLM's `GET /key/info` without `key` parameter describes the calling key: spend, limit and next reset.
/// With `model` (`<tenant>/<model>`) the gateway answers with the budget of that tenant's access (`?model=`).
pub fn budget(endpoint: &str, key: &str, model: Option<&str>, timeout: Duration) -> Result<KeyBudget, CheckResult> {
    parse_budget(&get(&with_model_query("key/info", model), endpoint, key, timeout)?, "info")
}

/// Budget for an SSO token: LiteLLM only finds a key for a JWT mapped to a virtual key; otherwise `/key/info` fails (404)
/// and `GET /user/info` without `user_id` describes the signed-in user: spend, limit and next reset.
/// `model` asks for the tenant of that model on both paths, like in `budget`.
pub fn sso_budget(endpoint: &str, token: &str, model: Option<&str>, timeout: Duration) -> Result<KeyBudget, CheckResult> {
    match budget(endpoint, token, model, timeout) {
        Err(CheckResult::HttpError(_)) => parse_budget(&get(&with_model_query("user/info", model), endpoint, token, timeout)?, "user_info"),
        other => other,
    }
}

/// `path` with `?model=<form-urlencoded>`; a missing or blank model leaves the path alone.
fn with_model_query(path: &str, model: Option<&str>) -> String {
    match model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => format!("{path}?model={}", url::form_urlencoded::byte_serialize(model.as_bytes()).collect::<String>()),
        None => path.to_string(),
    }
}

fn parse_budget(body: &str, field: &str) -> Result<KeyBudget, CheckResult> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| CheckResult::HttpError(200))?;
    let info = value.get(field).filter(|i| i.is_object()).ok_or(CheckResult::HttpError(200))?;
    Ok(KeyBudget {
        spend: info["spend"].as_f64().unwrap_or(0.0),
        max_budget: info["max_budget"].as_f64(),
        reset_at: info["budget_reset_at"].as_str().map(String::from),
    })
}

/// Remaining budget for `keyzapper-helper pool`: None if unknown, 0 if exhausted, infinite without limit.
pub fn remaining_budget(endpoint: &str, key: &str) -> Option<f64> {
    match budget(endpoint, key, None, Duration::from_secs(5)) {
        Ok(b) => Some(b.remaining()),
        Err(CheckResult::BudgetExceeded) => Some(0.0),
        Err(_) => None,
    }
}

/// Connection test; also reports configured models the key may not use.
pub fn run(endpoint: &str, key: &str, wanted: &[String]) -> CheckResult {
    match models(endpoint, key) {
        Err(failure) => failure,
        Ok(_) if wanted.is_empty() => CheckResult::Ok { missing_models: None },
        Ok(available) => CheckResult::Ok { missing_models: Some(wanted.iter().filter(|w| !available.contains(w)).cloned().collect()) },
    }
}
