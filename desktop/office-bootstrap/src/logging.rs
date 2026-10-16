//! Structured request logging with central redaction. Only whitelisted fields are logged: request ID,
//! pseudonymous user, tenant, Office host, profile ID, status, latency and provider kind. Tokens, headers and
//! bootstrap bodies never reach a log line; `redact` additionally scrubs token-shaped text from error messages.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Text,
}

/// Installs the global subscriber; `RUST_LOG` overrides the default `info` level.
pub fn init(format: LogFormat) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
    let result = match format {
        LogFormat::Json => builder.json().flatten_event(true).try_init(),
        LogFormat::Text => builder.try_init(),
    };
    // A second init (tests) is harmless.
    let _ = result;
}

/// Removes bearer tokens, API keys and JWTs from free text before it is logged.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let trimmed = word.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | ')' | '('));
        let lower = trimmed.to_lowercase();
        let secret = lower.starts_with("sk-")
            || lower.starts_with("eyj") && trimmed.matches('.').count() >= 2
            || lower.starts_with("bearer=")
            || lower.starts_with("authorization=")
            || lower.starts_with("x-api-key=");
        if secret {
            out.push_str("<redacted>");
        } else {
            out.push_str(word);
        }
    }
    // `Bearer <token>` spans two words; drop the word after it as well.
    let mut words: Vec<&str> = out.split(' ').collect();
    let mut i = 0;
    while i + 1 < words.len() {
        if words[i].eq_ignore_ascii_case("bearer") {
            words[i + 1] = "<redacted>";
        }
        i += 1;
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn scrubs_tokens_and_keys() {
        let text = "gateway said: Authorization: Bearer eyJhbGciOi.payload.sig and key sk-live-123 (status 401)";
        let redacted = redact(text);
        assert!(!redacted.contains("eyJ") && !redacted.contains("sk-live"), "{redacted}");
        assert!(redacted.contains("status 401") && redacted.contains("<redacted>"));
        assert_eq!(redact("connection refused to https://litellm.example"), "connection refused to https://litellm.example");
    }
}
