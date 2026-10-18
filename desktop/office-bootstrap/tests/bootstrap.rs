//! End-to-end tests of the bootstrap endpoint with locally generated RSA keys and a static JWKS: no Entra, no
//! network. Covers token validation, authorization, CORS, response shape and error mapping.

use axum::body::Body;
use axum::http::header::{ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, AUTHORIZATION, CACHE_CONTROL, ORIGIN};
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use http_body_util::BodyExt;
use jsonwebtoken::jwk::{AlgorithmParameters, CommonParameters, Jwk, JwkSet, KeyAlgorithm, PublicKeyUse, RSAKeyParameters, RSAKeyType};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use keyzapper_core::models::{McpServer, McpTransport, Profile};
use keyzapper_core::office::{
    AssignmentResolver, CredentialError, EntraIdentity, OfficeCredential, OfficeCredentialProvider, OfficeDeployment, OfficeDeploymentSettings, OfficeHost, OfficeSettings, StaticKeyProvider,
    UnassignedPolicy, DEFAULT_ALLOWED_ORIGIN,
};
use keyzapper_office_bootstrap::auth::{JwtVerifier, StaticJwks};
use keyzapper_office_bootstrap::cors::CorsPolicy;
use keyzapper_office_bootstrap::{router, ServerState};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tower::ServiceExt;

const TENANT: &str = "99999999-9999-9999-9999-999999999999";
const OTHER_TENANT: &str = "88888888-8888-8888-8888-888888888888";
const AUDIENCE: &str = "c2995f31-11e7-4882-b7a7-ef9def0a0266";
const ALPHA: &str = "11111111-1111-1111-1111-111111111111";
const BETA: &str = "22222222-2222-2222-2222-222222222222";
const USER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const OUTLOOK_USER: &str = "aaaaaaaa-bbbb-cccc-dddd-ffffffffffff";
const STRANGER: &str = "12345678-1234-1234-1234-123456789abc";
const REMOTE_MCP: &str = "33333333-3333-3333-3333-333333333333";
const LOCAL_MCP: &str = "44444444-4444-4444-4444-444444444444";

struct TestKey {
    kid: &'static str,
    pem: String,
    jwk: Jwk,
}

fn generate(kid: &'static str) -> TestKey {
    let key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let pem = key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap().to_string();
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_algorithm: Some(KeyAlgorithm::RS256),
            key_id: Some(kid.to_string()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
            key_type: RSAKeyType::RSA,
            n: URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),
            e: URL_SAFE_NO_PAD.encode(key.e().to_bytes_be()),
        }),
    };
    TestKey { kid, pem, jwk }
}

/// Key generation is slow in debug builds; one pair per test binary.
fn keys() -> &'static (TestKey, TestKey) {
    static KEYS: OnceLock<(TestKey, TestKey)> = OnceLock::new();
    KEYS.get_or_init(|| (generate("signing-key"), generate("rogue-key")))
}

fn now() -> i64 {
    keyzapper_core::oidc::unix_now()
}

fn claims(oid: &str) -> Value {
    json!({
        "iss": format!("https://login.microsoftonline.com/{TENANT}/v2.0"),
        "aud": AUDIENCE,
        "exp": now() + 600,
        "nbf": now() - 60,
        "iat": now() - 60,
        "oid": oid,
        "tid": TENANT,
        "scp": "access_as_user",
        "preferred_username": "someone@example.test",
    })
}

fn sign_with(key: &TestKey, kid: &str, claims: &Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    encode(&header, claims, &EncodingKey::from_rsa_pem(key.pem.as_bytes()).unwrap()).unwrap()
}

fn token(claims: &Value) -> String {
    let (signing, _) = keys();
    sign_with(signing, signing.kid, claims)
}

fn profile(id: &str, name: &str, users: &[&str], hosts: Vec<OfficeHost>) -> Profile {
    let mut p = Profile::new(id.into(), name.into(), "https://litellm.example.test/".into(), String::new());
    p.sonnet_model = Some("company-sonnet".into());
    p.office = Some(OfficeSettings {
        enabled: true,
        allowed_hosts: hosts,
        allowed_users: users.iter().map(|u| u.to_string()).collect(),
        inference_headers: BTreeMap::from([("x-keyzapper-profile".into(), "{{profile-name}}".into()), ("x-api-key".into(), "never".into())]),
        mcp_server_ids: vec![REMOTE_MCP.into(), LOCAL_MCP.into()],
        bootstrap_ttl_seconds: Some(300),
        ..OfficeSettings::default()
    });
    p
}

fn mcp_servers() -> Vec<McpServer> {
    let mut remote = McpServer::new(REMOTE_MCP.into(), "search".into());
    remote.transport = McpTransport::Http;
    remote.url = "https://mcp.example.test/search".into();
    remote.headers = BTreeMap::from([("X-Team".into(), "dev".into()), ("Authorization".into(), "Bearer secret".into())]);
    let mut local = McpServer::new(LOCAL_MCP.into(), "github".into());
    local.command = "npx".into();
    vec![remote, local]
}

fn deployment() -> OfficeDeployment {
    let state = keyzapper_core::models::AppState {
        profiles: vec![
            profile(ALPHA, "Consulting", &[USER], vec![OfficeHost::Excel, OfficeHost::Word, OfficeHost::PowerPoint]),
            profile(BETA, "Mail", &[OUTLOOK_USER], vec![OfficeHost::Outlook]),
        ],
        mcp_servers: mcp_servers(),
        office_deployment: Some(OfficeDeploymentSettings { tenant_id: TENANT.into(), audience: AUDIENCE.into(), bootstrap_url: "https://kz.example.test/office/bootstrap".into() }),
        ..Default::default()
    };
    OfficeDeployment::from_state(&state).unwrap()
}

struct FailingProvider(CredentialError);

impl OfficeCredentialProvider for FailingProvider {
    fn kind(&self) -> &'static str {
        "failing"
    }
    fn credential_for(&self, _: &Profile, _: &EntraIdentity, _: u64) -> Result<OfficeCredential, CredentialError> {
        Err(self.0.clone())
    }
}

fn app_with(provider: Box<dyn OfficeCredentialProvider>, policy: UnassignedPolicy) -> Router {
    let (signing, _) = keys();
    let deployment = deployment();
    let verifier = JwtVerifier::new(
        deployment.issuer(),
        deployment.tenant_id.clone(),
        deployment.audiences.clone(),
        vec![],
        Box::new(StaticJwks(JwkSet { keys: vec![signing.jwk.clone()] })),
        Duration::from_secs(3600),
    );
    router(Arc::new(ServerState {
        verifier,
        resolver: Box::new(AssignmentResolver::new(deployment.profiles.clone(), deployment.assignments.clone())),
        provider,
        cors: CorsPolicy::new(deployment.allowed_origins.clone()),
        mcp_servers: deployment.mcp_servers.clone(),
        unassigned_policy: policy,
        tenant_id: deployment.tenant_id.clone(),
        log_salt: "test".into(),
    }))
}

fn app() -> Router {
    app_with(Box::new(StaticKeyProvider::new(BTreeMap::from([(ALPHA.to_string(), "sk-vended-alpha".to_string()), (BETA.to_string(), "sk-vended-beta".to_string())]))), UnassignedPolicy::Forbid)
}

async fn call(app: Router, method: Method, bearer: Option<&str>, host: Option<&str>, origin: Option<&str>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder().method(method).uri("/office/bootstrap");
    if let Some(bearer) = bearer {
        request = request.header(AUTHORIZATION, bearer);
    }
    if let Some(host) = host {
        request = request.header("X-Claude-User-Agent", host);
    }
    if let Some(origin) = origin {
        request = request.header(ORIGIN, origin);
    }
    let response = app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into())) };
    (status, headers, body)
}

async fn get(bearer: Option<String>, host: &str) -> (StatusCode, axum::http::HeaderMap, Value) {
    call(app(), Method::GET, bearer.as_deref(), Some(host), Some(DEFAULT_ALLOWED_ORIGIN)).await
}

#[tokio::test]
async fn valid_token_gets_sparse_config_with_cors_and_no_store() {
    let bearer = format!("Bearer {}", token(&claims(USER)));
    let (status, headers, body) = get(Some(bearer), "claude-excel/1.4.0").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), DEFAULT_ALLOWED_ORIGIN);
    assert_eq!(headers.get(CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(body["gateway_url"], "https://litellm.example.test");
    assert_eq!(body["gateway_token"], "sk-vended-alpha");
    assert_eq!(body["gateway_api_format"], "anthropic");
    assert!(body.get("gateway_auth_header").is_none() && body.get("skills").is_none() && body.get("disabled_features").is_none());
    assert_eq!(body["inference_headers"], json!({"x-keyzapper-profile": "Consulting"}));
    assert_eq!(body["mcp_servers"], json!([{"url": "https://mcp.example.test/search", "label": "search", "headers": {"X-Team": "dev"}}]));
    assert_eq!(body["available_models"], json!(["company-sonnet"]));
    let expires = body["bootstrap_expires_at"].as_i64().unwrap();
    assert!((expires - now() - 300).abs() <= 5, "{expires}");
    let text = body.to_string();
    assert!(!text.contains("Bearer secret") && !text.contains("npx") && !text.contains("never"));
}

#[tokio::test]
async fn word_and_powerpoint_are_allowed_and_outlook_is_not() {
    for host in ["claude-word/1", "claude-powerpoint/1"] {
        let (status, _, _) = get(Some(format!("Bearer {}", token(&claims(USER)))), host).await;
        assert_eq!(status, StatusCode::OK, "{host}");
    }
    let (status, _, body) = get(Some(format!("Bearer {}", token(&claims(USER)))), "claude-outlook/1").await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")));
    let (status, _, _) = get(Some(format!("Bearer {}", token(&claims(OUTLOOK_USER)))), "claude-outlook/1").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get(Some(format!("Bearer {}", token(&claims(OUTLOOK_USER)))), "claude-excel/1").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for host in ["claude-teams/1", "Mozilla/5.0", ""] {
        let (status, _, _) = get(Some(format!("Bearer {}", token(&claims(USER)))), host).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{host:?}");
    }
    let (status, _, _) = call(app(), Method::GET, Some(&format!("Bearer {}", token(&claims(USER)))), None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_tokens_are_rejected_with_401() {
    let (signing, rogue) = keys();
    let mut cases: Vec<(&str, Option<String>)> = vec![("missing header", None), ("not bearer", Some("Basic abc".into())), ("garbage", Some("Bearer not.a.jwt".into()))];
    let mut wrong_aud = claims(USER);
    wrong_aud["aud"] = "other-app".into();
    cases.push(("wrong audience", Some(format!("Bearer {}", token(&wrong_aud)))));
    let mut wrong_iss = claims(USER);
    wrong_iss["iss"] = format!("https://login.microsoftonline.com/{OTHER_TENANT}/v2.0").into();
    cases.push(("wrong issuer", Some(format!("Bearer {}", token(&wrong_iss)))));
    let mut v1_iss = claims(USER);
    v1_iss["iss"] = format!("https://sts.windows.net/{TENANT}/").into();
    cases.push(("v1 issuer", Some(format!("Bearer {}", token(&v1_iss)))));
    let mut expired = claims(USER);
    expired["exp"] = (now() - 600).into();
    cases.push(("expired", Some(format!("Bearer {}", token(&expired)))));
    let mut future = claims(USER);
    future["nbf"] = (now() + 600).into();
    cases.push(("not yet valid", Some(format!("Bearer {}", token(&future)))));
    let mut no_oid = claims(USER);
    no_oid.as_object_mut().unwrap().remove("oid");
    cases.push(("missing oid", Some(format!("Bearer {}", token(&no_oid)))));
    let mut bad_oid = claims(USER);
    bad_oid["oid"] = "not-a-guid".into();
    cases.push(("malformed oid", Some(format!("Bearer {}", token(&bad_oid)))));
    let mut other_tenant = claims(USER);
    other_tenant["tid"] = OTHER_TENANT.into();
    cases.push(("other tenant", Some(format!("Bearer {}", token(&other_tenant)))));
    cases.push(("bad signature", Some(format!("Bearer {}", sign_with(rogue, signing.kid, &claims(USER))))));
    cases.push(("unknown kid", Some(format!("Bearer {}", sign_with(rogue, rogue.kid, &claims(USER))))));
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(signing.kid.to_string());
    let hs256 = encode(&header, &claims(USER), &EncodingKey::from_secret(b"n")).unwrap();
    cases.push(("hs256", Some(format!("Bearer {hs256}"))));
    for (name, bearer) in cases {
        let (status, headers, body) = get(bearer, "claude-excel/1").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}: {body}");
        assert_eq!(body["error"], "unauthorized", "{name}");
        assert_eq!(headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), DEFAULT_ALLOWED_ORIGIN, "{name}");
    }
}

#[tokio::test]
async fn unknown_users_follow_the_policy() {
    let bearer = format!("Bearer {}", token(&claims(STRANGER)));
    let (status, _, body) = get(Some(bearer.clone()), "claude-excel/1").await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")));
    let defaults = app_with(Box::new(StaticKeyProvider::new(BTreeMap::new())), UnassignedPolicy::Defaults);
    let (status, _, body) = call(defaults, Method::GET, Some(&bearer), Some("claude-excel/1"), None).await;
    assert_eq!((status, body), (StatusCode::OK, json!({})));
}

#[tokio::test]
async fn disabled_office_profile_is_forbidden() {
    let (signing, _) = keys();
    let mut disabled = profile(ALPHA, "Off", &[USER], vec![OfficeHost::Excel]);
    disabled.office.as_mut().unwrap().enabled = false;
    let verifier = JwtVerifier::new(
        format!("https://login.microsoftonline.com/{TENANT}/v2.0"),
        TENANT.into(),
        vec![AUDIENCE.into()],
        vec![],
        Box::new(StaticJwks(JwkSet { keys: vec![signing.jwk.clone()] })),
        Duration::from_secs(60),
    );
    let app = router(Arc::new(ServerState {
        verifier,
        resolver: Box::new(AssignmentResolver::new(vec![disabled], BTreeMap::from([(USER.to_string(), ALPHA.to_string())]))),
        provider: Box::new(StaticKeyProvider::new(BTreeMap::from([(ALPHA.to_string(), "sk-x".to_string())]))),
        cors: CorsPolicy::default(),
        mcp_servers: vec![],
        unassigned_policy: UnassignedPolicy::Forbid,
        tenant_id: TENANT.into(),
        log_salt: String::new(),
    }));
    let (status, _, _) = call(app, Method::GET, Some(&format!("Bearer {}", token(&claims(USER)))), Some("claude-excel/1"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn required_scope_is_enforced() {
    let (signing, _) = keys();
    let verifier = JwtVerifier::new(
        format!("https://login.microsoftonline.com/{TENANT}/v2.0"),
        TENANT.into(),
        vec![AUDIENCE.into()],
        vec!["access_as_user".into()],
        Box::new(StaticJwks(JwkSet { keys: vec![signing.jwk.clone()] })),
        Duration::from_secs(60),
    );
    let identity = verifier.verify(&token(&claims(USER))).unwrap();
    assert_eq!((identity.oid.as_str(), identity.tenant_id.as_str()), (USER, TENANT));
    assert_eq!(identity.scopes, ["access_as_user"]);
    let mut other_scope = claims(USER);
    other_scope["scp"] = "something.else".into();
    assert!(verifier.verify(&token(&other_scope)).is_err());
    let upper = claims(USER.to_uppercase().as_str());
    assert_eq!(verifier.verify(&token(&upper)).unwrap().oid, USER);
}

#[tokio::test]
async fn credential_provider_failures_map_to_503_and_500() {
    let bearer = format!("Bearer {}", token(&claims(USER)));
    let unavailable = app_with(Box::new(FailingProvider(CredentialError::Unavailable("litellm down sk-leak".into()))), UnassignedPolicy::Forbid);
    let (status, headers, body) = call(unavailable, Method::GET, Some(&bearer), Some("claude-excel/1"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("credential_provider_unavailable")));
    assert_eq!(headers.get("retry-after").unwrap(), "10");
    assert!(!body.to_string().contains("sk-leak"));
    let misconfigured = app_with(Box::new(FailingProvider(CredentialError::NotConfigured("no key".into()))), UnassignedPolicy::Forbid);
    let (status, _, body) = call(misconfigured, Method::GET, Some(&bearer), Some("claude-excel/1"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("internal")));
    let no_key = app_with(Box::new(StaticKeyProvider::new(BTreeMap::new())), UnassignedPolicy::Forbid);
    let (status, _, _) = call(no_key, Method::GET, Some(&bearer), Some("claude-excel/1"), None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn cors_preflight_and_foreign_origins() {
    let (status, headers, _) = call(app(), Method::OPTIONS, None, None, Some(DEFAULT_ALLOWED_ORIGIN)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), DEFAULT_ALLOWED_ORIGIN);
    assert_eq!(headers.get(ACCESS_CONTROL_ALLOW_METHODS).unwrap(), "GET");
    let allowed = headers.get(ACCESS_CONTROL_ALLOW_HEADERS).unwrap().to_str().unwrap().to_lowercase();
    assert!(allowed.contains("authorization") && allowed.contains("x-claude-user-agent"));
    let (status, headers, _) = call(app(), Method::OPTIONS, None, None, Some("https://evil.example")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    let bearer = format!("Bearer {}", token(&claims(USER)));
    let (status, headers, _) = call(app(), Method::GET, Some(&bearer), Some("claude-excel/1"), Some("https://evil.example")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    assert!(headers.values().all(|v| v != "*"));
}

#[tokio::test]
async fn health_needs_no_token() {
    let response = app().oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
