//! `keyzapper-office-bootstrap`: the HTTPS endpoint Claude for Microsoft 365 calls at add-in start
//! (`GET /office/bootstrap` with the user's Microsoft Entra token). It validates the token, maps the user (`oid`)
//! to a KeyZapper profile, vends a gateway credential and answers with the add-in's sparse configuration.
//!
//! Runs without the desktop app: it reads a deployment file the app exports (no secrets) plus server-side
//! credential configuration (environment or owner-only files). TLS termination is left to the reverse proxy in
//! front of it; the service itself listens on plain HTTP, by default on loopback only.

pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod cors;
pub mod logging;

pub use bootstrap::{router, ServerState};
