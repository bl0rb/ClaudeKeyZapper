//! `keyzapper-office-bootstrap --config /etc/keyzapper/office.json` (or `KEYZAPPER_OFFICE_CONFIG=…`).
//! `--check` validates the configuration (including secret sources) and exits. No secret is ever an argument.

use keyzapper_office_bootstrap::auth::{HttpJwks, JwtVerifier};
use keyzapper_office_bootstrap::config::{ServerConfig, CONFIG_ENV};
use keyzapper_office_bootstrap::cors::CorsPolicy;
use keyzapper_office_bootstrap::{logging, router, ServerState};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "usage: keyzapper-office-bootstrap [--config <file>] [--check]\n       (or set KEYZAPPER_OFFICE_CONFIG=<file>)";

fn main() {
    let mut args = std::env::args().skip(1);
    let mut config_path: Option<PathBuf> = std::env::var_os(CONFIG_ENV).map(PathBuf::from);
    let mut check_only = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => config_path = args.next().map(PathBuf::from),
            "--check" => check_only = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            _ => {
                eprintln!("unknown argument\n{USAGE}");
                std::process::exit(64);
            }
        }
    }
    let Some(config_path) = config_path else {
        eprintln!("{USAGE}");
        std::process::exit(64);
    };
    // Parse first so the log format is known before anything is logged.
    let format = std::fs::read_to_string(&config_path).ok().and_then(|t| ServerConfig::parse(&t).ok()).map(|c| c.log_format).unwrap_or_default();
    logging::init(format);
    let loaded = match ServerConfig::load(&config_path) {
        Ok(loaded) => loaded,
        Err(e) => {
            tracing::error!(error = %logging::redact(&e), "configuration rejected");
            std::process::exit(78);
        }
    };
    let deployment = loaded.deployment;
    let jwks_url = loaded.config.jwks_url.clone().unwrap_or_else(|| deployment.jwks_url());
    let verifier = JwtVerifier::new(
        deployment.issuer(),
        deployment.tenant_id.clone(),
        deployment.audiences.clone(),
        loaded.config.required_scopes.clone(),
        Box::new(HttpJwks { url: jwks_url, timeout: Duration::from_secs(10) }),
        Duration::from_secs(loaded.config.jwks_cache_seconds.max(60)),
    );
    let state = Arc::new(ServerState {
        verifier,
        resolver: Box::new(deployment.resolver()),
        provider: loaded.provider,
        cors: CorsPolicy::new(deployment.allowed_origins.clone()),
        mcp_servers: deployment.mcp_servers.clone(),
        unassigned_policy: deployment.unassigned_policy,
        tenant_id: deployment.tenant_id.clone(),
        log_salt: loaded.log_salt,
    });
    tracing::info!(
        tenant = %deployment.tenant_id,
        profiles = deployment.profiles.len(),
        assignments = deployment.assignments.len(),
        origins = ?state.cors.origins(),
        provider = state.provider.kind(),
        "configuration loaded"
    );
    if check_only {
        println!("configuration ok");
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(&loaded.config.listen).await {
            Ok(listener) => listener,
            Err(e) => {
                tracing::error!(listen = %loaded.config.listen, error = %e, "cannot bind");
                std::process::exit(71);
            }
        };
        tracing::info!(listen = %loaded.config.listen, "listening");
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        };
        if let Err(e) = axum::serve(listener, router(state)).with_graceful_shutdown(shutdown).await {
            tracing::error!(error = %e, "server failed");
            std::process::exit(70);
        }
    });
}
