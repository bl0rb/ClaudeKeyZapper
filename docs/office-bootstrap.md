# Claude for Microsoft 365 via KeyZapper bootstrap

KeyZapper profiles can serve Claude for Excel, Word, PowerPoint (and, optionally, Outlook) in addition to Claude Code.
The Office add-in runs in a browser sandbox, so it cannot call `keyzapper-helper`. Instead it uses Anthropic's
official third-party path: at start it fetches its configuration from an HTTPS **bootstrap endpoint** with the
user's Microsoft Entra token. `keyzapper-office-bootstrap` is that endpoint. It validates the token, maps the user to
a KeyZapper profile and answers with the gateway URL, a short-lived gateway token, inference headers and the
profile's remote MCP servers.

```
Office Add-in
     │
     │ Entra JWT
     ▼
KeyZapper Bootstrap  (keyzapper-office-bootstrap)
     │
     ├── JWT validation (signature, iss, aud, exp, tid, oid)
     ├── oid → profile
     ├── policy (Office enabled? host allowed?)
     └── credential vending
              │
              ▼
            LiteLLM
              │
              ▼
       Bedrock / Provider
```

Both paths use the same profile:

```
                         KeyZapper
                            │
                      Profile Model
                            │
             ┌──────────────┴──────────────┐
             │                             │
        Claude Code                 Claude for Office
             │                             │
       apiKeyHelper                  HTTPS Bootstrap
             │                             │
   keyzapper-helper                  Entra JWT
             │                             │
             └──────────────┬──────────────┘
                            │
                            ▼
                         LiteLLM
```

The integration uses only the bootstrap and gateway interfaces Anthropic documents. It never touches the add-in's
browser storage, sessions or cookies. Office is **off by default**: a profile serves Office only after an
administrator releases it in the app, and no key reaches an Office client without that.

## Components

| Piece | Where | Role |
|---|---|---|
| Office settings per profile | KeyZapper app, profile editor, section *Claude for Microsoft 365* | Release, allowed Office apps, Entra users, inference headers, MCP servers, lifetime |
| Deployment file | *Backup › Export Office bootstrap configuration…* | Profiles released for Office, their remote MCP servers, user assignments, tenant, audience. **No secrets.** |
| `keyzapper-office-bootstrap` | `desktop/office-bootstrap`, runs centrally | `GET /office/bootstrap`, `OPTIONS /office/bootstrap`, `GET /health` |
| Credential provider | Server configuration | Where the add-in's gateway token comes from (LiteLLM virtual keys or a static key file) |
| Add-in manifest | Anthropic's `claude-for-msft-365-install` tooling | `entra_sso=1`, `bootstrap_url=https://…/office/bootstrap` |

## Request flow

```
GET https://keyzapper.company.example/office/bootstrap
Authorization: Bearer <entra-jwt>
X-Claude-User-Agent: claude-excel/<version>
Origin: https://pivot.claude.ai
```

1. **CORS.** The task pane's origin is `https://pivot.claude.ai`; the server echoes exactly that origin (configurable,
   never `*`) and answers the `OPTIONS` preflight with `Access-Control-Allow-Methods: GET` and
   `Access-Control-Allow-Headers: Authorization, X-Claude-User-Agent, *`.
2. **Token.** Signature against the tenant's JWKS (`https://login.microsoftonline.com/<tenant>/discovery/v2.0/keys`,
   cached, refreshed on unknown `kid`), `iss` = `https://login.microsoftonline.com/<tenant>/v2.0`, `aud` from the
   configuration, `exp`/`nbf` with 60 s leeway, `tid` must be the tenant, `oid` must be a GUID. Only RS256. Any failure → `401`.
3. **Host.** `X-Claude-User-Agent` → `excel`, `word`, `powerpoint`, `outlook`; anything else is unknown and never served.
4. **Profile.** Explicit `assignments` (`oid` → profile) first, then the profiles' own user lists. The profile must be
   released for Office and allow the host; otherwise `403`.
5. **Credential.** The configured provider issues the token for this user and profile.
6. **Response** (sparse: only set keys), `Cache-Control: no-store`:

```json
{
  "gateway_url": "https://litellm.company.example",
  "gateway_token": "<short-lived token>",
  "gateway_api_format": "anthropic",
  "inference_headers": { "x-keyzapper-profile": "Consulting EU", "x-keyzapper-cost-center": "4711" },
  "mcp_servers": [{ "url": "https://mcp.company.example/search", "label": "Internal Search" }],
  "available_models": ["company-claude-sonnet"],
  "bootstrap_expires_at": 1792152300
}
```

`gateway_auth_header` is included only when the profile uses `authorization` instead of the default `x-api-key`.
`disabled_features` and `skills` are part of the format but empty unless configured. The add-in re-fetches the
configuration shortly before `bootstrap_expires_at` (default 15 minutes) and after a `401` from the gateway, so a vended
key only has to live that long.

| Status | Meaning |
|---|---|
| `200` | Configuration for the user |
| `200 {}` | Valid user without a profile, with `unassignedPolicy: "defaults"` (manifest defaults apply) |
| `401` | Missing or invalid token (`{"error":"unauthorized"}`) |
| `403` | Valid user, but no profile, profile not released, or Office app not allowed |
| `500` | Credential provider misconfigured (no key for the profile, management key rejected) |
| `503` | Credential provider temporarily unavailable (`Retry-After: 10`) |

Error bodies carry only the code; details go to the log.

## Credential vending

The add-in needs a bearer token for the gateway. Two providers exist behind the `OfficeCredentialProvider` interface:

**`litellmVirtualKey` (recommended).** For every bootstrap the server calls `POST <gateway>/key/generate` with a
LiteLLM management key and `duration` = the profile's lifetime, so every Office user gets a short-lived key of their
own. The key inherits the profile's models and carries `metadata.keyzapper_profile` and a pseudonymous
`metadata.keyzapper_user`, so LiteLLM's spend logs stay attributable without e-mail addresses. Optional `maxBudget`
caps each key. The management key comes from an environment variable or an owner-only file, never from the
configuration file or the command line.

**`staticProfileKeys`.** The profiles' long-lived keys from an owner-only JSON file `{"<profile-id>": "sk-…"}`. Every
Office user of a profile then shares that key, and it stays valid until rotated in LiteLLM. Use this only where
virtual keys are impossible; the server logs a warning at start. Even here the key is never logged, never in
telemetry and never in an error message.

Further providers (OIDC token exchange, external secret stores) can be added without changing the protocol.

## Setting it up

### 1. Release profiles in KeyZapper

In the profile editor, section *Claude for Microsoft 365*:

* **Release for Claude for Microsoft 365** – off by default.
* **Allowed applications** – Excel, Word, PowerPoint; Outlook is off by default (Outlook needs its own manifest and
  Graph consent; treat it as experimental).
* **Entra users** – object IDs (`oid`), one per line. Not e-mail addresses: those change and are not in every token.
* **Inference headers** – `Name: value` lines the add-in adds to every request, e.g. a cost centre.
  `{{profile-id}}` and `{{profile-name}}` are substituted. Reserved names (`authorization`, `x-api-key`,
  `content-type`, `host`, `content-length`, `user-agent`, `cookie`, `anthropic-*`, `x-amz-*`, `x-goog-*`) are rejected.
  Never put secrets here: every Office user receives them.
* **MCP servers** – the existing KeyZapper MCP definitions. Only remote servers (HTTP/SSE over `https`) are offered;
  local stdio servers and credential headers (`Authorization`, `x-api-key`, `${…}` placeholders) never reach the add-in.
* **Lifetime** – minutes a bootstrap response and its key are valid (1 to 1440, default 15).
* **Token header** – `x-api-key` (default) or `Authorization: Bearer`, matching what the gateway expects.

### 2. Export the deployment file

*Backup › Export Office bootstrap configuration…* asks for

* the **Entra tenant ID** (GUID),
* the **audience**: the `aud` of the add-in's token. With Anthropic's add-in that is its app ID
  `c2995f31-11e7-4882-b7a7-ef9def0a0266`; with your own Entra app (`graph_client_id` in the manifest) its client ID, or
  `api://<client-id>` when `entra_scope` is set,
* the **bootstrap URL** under which the service will be reachable.

and writes `keyzapper-office-deployment-<date>.json`: schema version, tenant, audiences, allowed origins
(`https://pivot.claude.ai`), released profiles with their Office settings, the referenced remote MCP servers
(without credential headers), and `assignments` derived from the user lists (first profile wins; edit on the server
if needed). The file contains no keys, tokens or secrets of any kind and is safe to keep in configuration management.

### 3. Configure the server

`/etc/keyzapper/office.json`:

```json
{
  "listen": "127.0.0.1:8787",
  "deploymentFile": "office-deployment.json",
  "credentialProvider": {
    "type": "litellmVirtualKey",
    "managementKeyEnv": "KEYZAPPER_LITELLM_MANAGEMENT_KEY",
    "maxBudget": 5
  },
  "requiredScopes": [],
  "logSaltEnv": "KEYZAPPER_LOG_SALT",
  "logFormat": "json"
}
```

| Key | Meaning |
|---|---|
| `listen` | Bind address; loopback by default. Terminate TLS in a reverse proxy in front (nginx, Caddy, API gateway). |
| `deployment` / `deploymentFile` | The exported file, inline or by path (relative to the configuration file). |
| `credentialProvider` | `litellmVirtualKey` with `managementKeyEnv` or `managementKeyFile` (optional `endpoint`, `maxBudget`), or `staticProfileKeys` with `keysFile`. |
| `jwksUrl` | Override of the tenant's JWKS URL (mirrors, tests). |
| `jwksCacheSeconds` | JWKS cache lifetime (default 3600; unknown `kid` refreshes earlier, at most every 30 s). |
| `requiredScopes` | `scp` values the token must contain, e.g. when the manifest sets `entra_scope`. |
| `logSaltEnv` | Environment variable with the salt for pseudonymous user IDs in logs and key metadata; without it a random salt per process. |
| `logFormat` | `json` (default) or `text`. |

Unknown keys are rejected, so a typo cannot silently disable a check. Secrets are read only from the environment or
from files named in the configuration; the server warns when such a file is readable by other users.

Alternative static keys:

```json
"credentialProvider": { "type": "staticProfileKeys", "keysFile": "/etc/keyzapper/office-keys.json" }
```

with `office-keys.json` (mode `0600`) `{"11111111-1111-1111-1111-111111111111": "sk-…"}`.

### 4. Run it

```bash
cd desktop && cargo build --release -p keyzapper-office-bootstrap
```

```bash
KEYZAPPER_LITELLM_MANAGEMENT_KEY=… keyzapper-office-bootstrap --config /etc/keyzapper/office.json --check
```

`--check` loads everything including the secret sources and exits. Without `--check` the service listens until
`SIGINT`. `KEYZAPPER_OFFICE_CONFIG=<file>` replaces `--config`. Put it behind a reverse proxy that terminates TLS for
the public bootstrap URL and forwards `Authorization`, `Origin` and `X-Claude-User-Agent` unchanged. The endpoint
needs to be reachable from the users' browsers (intranet or zero-trust proxy is fine); it does not need to be public.

Example systemd unit:

```ini
[Service]
User=keyzapper
EnvironmentFile=/etc/keyzapper/office.env
ExecStart=/usr/local/bin/keyzapper-office-bootstrap --config /etc/keyzapper/office.json
Restart=on-failure
```

### 5. Point the add-in at it

Deploy the add-in with Anthropic's `claude-for-msft-365-install` plugin (`/claude-for-msft-365-install:manifest` or
`:setup`). The manifest for Excel, Word and PowerPoint needs at least

```
entra_sso=1
bootstrap_url=https://keyzapper.company.example/office/bootstrap
```

`gateway_url` may be set in the manifest as a fallback; the bootstrap response overrides it per user. Put nothing
secret in the manifest URL; if you must set a parameter like `gateway_token` there, place it after `#`, never after `?`.
Outlook uses a separate manifest (`manifest-outlook.xml`) and additional Microsoft Graph consent; follow Anthropic's
documentation for it and only then allow Outlook on a profile.

Users open Claude in Excel, Word or PowerPoint, sign in with Microsoft once, and get the configuration of their profile.
Changing the profile in KeyZapper, re-exporting and restarting the service is enough; the add-in picks it up at its
next load or before `bootstrap_expires_at`.

## LiteLLM

The add-in talks to `<gateway_url>/v1/messages` in Anthropic format (`gateway_api_format: anthropic`), the same way
Claude Code does. LiteLLM therefore needs:

* CORS for `https://pivot.claude.ai` on the gateway itself (the add-in calls it from the browser), allowing the
  `x-api-key` or `authorization` header, `anthropic-version`, `anthropic-beta`, `content-type` and your inference headers;
* `GET /v1/models` for model discovery, or `available_models` from the profile's model tiers;
* for `litellmVirtualKey`: a management key allowed to call `/key/generate`; the vended keys appear in LiteLLM's key
  list with `metadata.keyzapper_profile` and `metadata.keyzapper_user` and expire on their own.

## Logging

One structured line per request: `request_id`, pseudonymous `user`, `tenant`, `host`, `profile`, `status`,
`outcome`, `latency_ms`, `provider`, and for errors a short `detail` category. Never logged: `Authorization` headers,
JWTs, gateway tokens, API keys, MCP headers or response bodies. Error texts pass through a central redaction that
removes anything token-shaped before it is written; `BootstrapResponse` and `OfficeCredential` print as redacted
when formatted with `{:?}`.

## Security summary

* Signature, `iss`, `aud`, `exp`/`nbf`, tenant and `oid` are always validated; RS256 only; fail closed with `401`.
* `oid` is the only identity; names and e-mail addresses are never used for mapping.
* CORS echoes explicit origins only, never `*`.
* Office is off by default per profile; unknown Office hosts and unassigned users get `403`.
* Secrets live outside the configuration file and the command line; the exported deployment file contains none.
* Inference headers cannot carry credential headers; MCP credential headers and local servers never reach the add-in.
* Short-lived virtual keys are the default recommendation; static keys are an explicit, documented downgrade.
* `Cache-Control: no-store` on every bootstrap response.

## Troubleshooting

| Symptom | Check |
|---|---|
| Add-in shows a connection error, server logs nothing | Browser blocked by CORS: `Origin` must be `https://pivot.claude.ai`; the proxy must forward `OPTIONS` and the `Origin` header. Look at the task pane's network tab. |
| `401` with `detail: wrong audience` | The token's `aud` is not in `audiences`: Anthropic's app ID vs. your own `graph_client_id`, or `api://…` when `entra_scope` is set. |
| `401` with `wrong issuer` or `wrong tenant` | Token from another tenant, or a v1 issuer (`sts.windows.net`); the add-in must obtain v2 tokens. |
| `401` with `unknown signing key` | Key rotation; the server refreshes the JWKS on its own. Persisting: `jwksUrl` wrong or blocked. |
| `403` | User not in any released profile, or the Office app not allowed (Outlook is off by default). |
| `500` | Static key file lacks the profile, or LiteLLM rejected the management key. |
| `503` | LiteLLM unreachable or `/key/generate` failing; the add-in retries. |
| Models missing in the add-in | Set the profile's model tiers (they become `available_models`) or expose `/v1/models` on the gateway. |
| Changes in KeyZapper not visible | Re-export the deployment file and restart the service. |

## Tests

```bash
cd desktop && cargo test -p keyzapper-core -p keyzapper-office-bootstrap
```

The server tests generate RSA keys locally and run the full request path through the router: valid tokens, wrong
audience/issuer/tenant, expired and not-yet-valid tokens, missing or malformed `oid`, bad signatures, unknown keys,
HS256 downgrade, host rules per Office app, unassigned users with both policies, CORS preflight and foreign origins,
provider failures (`500`/`503`) and redaction. Nothing talks to Entra, LiteLLM or the network.
