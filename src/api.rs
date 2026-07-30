//! The JSON wire format of the dashboard API, as types.
//!
//! Every `/api/*` (and setup) response body is a struct in this module with
//! `derive(Serialize, ToSchema)`. Two things follow from that: the shape a
//! handler returns is readable without executing it, and `openapi.json` is
//! generated from the same declarations the handlers serialize — there is no
//! second, hand-maintained description of the API to drift.
//!
//! # Field order is the wire order
//!
//! Serde emits struct fields in **declaration order**. The hand-built
//! `serde_json::json!` bodies these types replaced emitted keys in **ASCII
//! order**, because `serde_json::Map` is a `BTreeMap` unless the crate's
//! `preserve_order` feature is on (it is not, here). So every struct below
//! declares its fields in ASCII order, which is what keeps the bytes on the
//! wire byte-for-byte identical to the pre-0.6.6 responses.
//!
//! Note that `_` (0x5F) sorts *before* every lowercase letter: `lane` <
//! `last4`, `username` < `users`. `api::field_order` guards the rule so a
//! future edit that reorders a field trips a test instead of a client.
//!
//! See `knowledge/decisions/typed-responses-and-generated-openapi.md`.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::{request::Parts, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};

use crate::config::{DashboardCfg, GovernorCfg, Limits, Mode, Role};
use crate::history::{HistoryDiagnostics, MetricValue, RollupPoint, Tail};

// ---------------------------------------------------------------------------
// Shared envelopes
// ---------------------------------------------------------------------------

/// The single error envelope every JSON endpoint answers failures with —
/// the same shape `/v1` uses, so one client-side branch handles both.
#[derive(Serialize, ToSchema)]
pub struct ApiError {
    pub error: ApiErrorBody,
}

#[derive(Serialize, ToSchema)]
pub struct ApiErrorBody {
    /// Machine-readable reason, e.g. `invalid_config`, `forbidden`.
    pub code: String,
    /// Operator-facing explanation; safe to show in the UI.
    pub message: String,
    /// Always `proxy_error` — the error came from nim-proxy, not upstream.
    #[serde(rename = "type")]
    #[schema(rename = "type", example = "proxy_error")]
    pub kind: &'static str,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            error: ApiErrorBody {
                code: code.to_owned(),
                message: message.into(),
                kind: "proxy_error",
            },
        }
    }
}

/// A JSON extractor whose failures use the dashboard API's stable error
/// envelope instead of Axum's framework-default text responses.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(JsonRejection::MissingJsonContentType(_)) => Err(api_error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "Content-Type must be application/json",
            )),
            Err(JsonRejection::BytesRejection(rejection)) => {
                let (status, code, message) = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE
                {
                    (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "body_too_large",
                        "request body is too large",
                    )
                } else {
                    (StatusCode::BAD_REQUEST, "invalid_json", "invalid JSON")
                };
                Err(api_error_response(status, code, message))
            }
            Err(JsonRejection::JsonDataError(_)) => Err(api_error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_json",
                "invalid JSON",
            )),
            Err(_) => Err(api_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "invalid JSON",
            )),
        }
    }
}

/// A query extractor whose failures use the dashboard API's stable error
/// envelope instead of Axum's framework-default text responses.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(_) => Err(api_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                "invalid query",
            )),
        }
    }
}

fn api_error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, axum::Json(ApiError::new(code, message))).into_response()
}

/// The protected `/api/*` fallback. It is deliberately not installed on page,
/// health, metrics, login/form, setup GET, or `/v1` routes.
pub async fn api_not_found() -> Response {
    api_error_response(StatusCode::NOT_FOUND, "not_found", "not found")
}

/// The protected `/api/*` method fallback. It has the same narrow scope as
/// [`api_not_found`].
pub async fn api_method_not_allowed() -> Response {
    api_error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "method not allowed",
    )
}

/// A settings write that applied. Success carries no data: the dashboard
/// re-reads `/api/config` rather than trusting an echo.
#[derive(Serialize, ToSchema)]
pub struct OkResponse {
    /// Always `true`; failures are an [`ApiError`] with a 4xx status.
    pub ok: bool,
}

impl OkResponse {
    pub fn new() -> Self {
        Self { ok: true }
    }
}

// ---------------------------------------------------------------------------
// GET /api/config
// ---------------------------------------------------------------------------

/// The Settings page's data source. `server` and `users` are absent — not
/// null — for non-admin callers: the filtering happens before serialization,
/// so DOM tampering reveals nothing.
#[derive(Serialize, ToSchema)]
pub struct ConfigResponse {
    pub client_keys: Vec<ClientKeyRow>,
    pub mode: Mode,
    pub nim_keys: Vec<NimKeyRow>,
    pub pool: PoolSummary,
    pub role: Role,
    /// Admin-only; omitted entirely for the `user` role.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub server: Option<ServerSettings>,
    pub username: String,
    /// Admin-only; omitted entirely for the `user` role.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub users: Option<Vec<UserRow>>,
}

/// A stored client key. The secret itself is never served back — only the
/// masked tail kept for display.
#[derive(Serialize, ToSchema)]
pub struct ClientKeyRow {
    pub last4: String,
    pub name: String,
    pub owner: String,
}

/// A stored NIM key plus its live lane state. `lane`, `in_window` and
/// `cooldown_ms` are null for a disabled key (it holds no lane).
#[derive(Serialize, ToSchema)]
pub struct NimKeyRow {
    pub cooldown_ms: Option<u64>,
    pub enabled: bool,
    /// First 8 hex chars of SHA-256(key) — the key's public identifier.
    pub fingerprint: String,
    /// True for the superuser's only enabled key: the pool floor, which
    /// `config::validate` refuses to remove or disable.
    pub guarded: bool,
    pub in_window: Option<usize>,
    pub lane: Option<usize>,
    pub last4: String,
    pub owner: String,
    pub rpm: usize,
}

/// Pool aggregate — visible to every role, since it carries no ownership.
#[derive(Serialize, ToSchema)]
pub struct PoolSummary {
    pub capacity_rpm: usize,
    pub enabled: usize,
}

/// Admin-only server settings, mirroring the stored config sections.
#[derive(Serialize, ToSchema)]
pub struct ServerSettings {
    pub base_url: String,
    pub dashboard: DashboardCfg,
    pub governor: GovernorCfg,
    pub history: HistorySettings,
    pub limits: Limits,
}

/// Retention setting plus the live state of the history file.
#[derive(Serialize, ToSchema)]
pub struct HistorySettings {
    pub available_from: Option<u64>,
    pub compaction_pending: bool,
    /// Retention in days; 0 = keep forever.
    pub days: u64,
    pub file_bytes: u64,
}

/// A user row with how much of the pool they own.
#[derive(Serialize, ToSchema)]
pub struct UserRow {
    pub client_keys: usize,
    pub nim_keys: usize,
    pub role: Role,
    pub username: String,
}

// ---------------------------------------------------------------------------
// POST /api/settings/*
// ---------------------------------------------------------------------------

/// `POST /api/settings/clients`. `secret` appears only on `add`, and only
/// this once — the store keeps a SHA-256 digest, never the secret.
#[derive(Serialize, ToSchema)]
pub struct ClientsResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub secret: Option<String>,
}

/// `POST /api/settings/validate-key` and `POST /setup/validate-key`.
/// `models` is the size of the upstream catalog the key could read;
/// `error` explains a refusal. Exactly one of the two is present.
#[derive(Serialize, ToSchema)]
pub struct ValidateKeyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub models: Option<usize>,
    pub ok: bool,
}

impl ValidateKeyResponse {
    pub fn probed(result: Result<usize, String>) -> Self {
        match result {
            Ok(models) => Self {
                error: None,
                models: Some(models),
                ok: true,
            },
            Err(error) => Self {
                error: Some(error),
                models: None,
                ok: false,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// POST /setup
// ---------------------------------------------------------------------------

/// The first-run claim. `client_key` is present when the wizard asked for a
/// first client key — the one and only time that secret leaves the server.
#[derive(Serialize, ToSchema)]
pub struct SetupResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub client_key: Option<MintedClientKey>,
    pub ok: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MintedClientKey {
    pub name: String,
    pub secret: String,
}

// ---------------------------------------------------------------------------
// GET /api/dashboard, GET /api/dashboard/now
// ---------------------------------------------------------------------------

/// Rolled-up history for one analytical window. `config_revision` and
/// `history_revision` let the dashboard skip a redraw when nothing moved.
#[derive(Serialize, ToSchema)]
pub struct DashboardResponse {
    pub config_revision: u64,
    pub diagnostics: HistoryDiagnostics,
    pub history_revision: u64,
    /// Latest value of each gauge in the window.
    pub latest: Vec<MetricValue>,
    pub points: Vec<RollupPoint>,
    /// Counter deltas summed across the window.
    pub totals: Vec<MetricValue>,
    pub window: DashboardWindow,
}

/// What was asked for, what could be served, and what exists on disk —
/// kept separate so the UI can say "you asked for 30d, I have 3d".
#[derive(Serialize, ToSchema)]
pub struct DashboardWindow {
    pub available_from: Option<u64>,
    pub available_to: Option<u64>,
    pub default_window_days: u64,
    pub effective_from: Option<u64>,
    pub effective_to: Option<u64>,
    /// True when the caller sent no `to`, i.e. the window tracks now.
    pub following_now: bool,
    pub requested_from: u64,
    pub requested_to: u64,
    pub retention_days: u64,
}

/// The "Now" strip: live registry values plus the configuration they were
/// sampled under. Deliberately not a slice of history — it is this instant.
#[derive(Serialize, ToSchema)]
pub struct DashboardNowResponse {
    /// True when `/v1` requires a client key (keyed mode).
    pub auth: bool,
    pub available_from: Option<u64>,
    pub available_to: Option<u64>,
    pub capacity_rpm: usize,
    pub config_revision: u64,
    pub default_window_days: u64,
    pub history_revision: u64,
    pub lanes: usize,
    pub metrics: Vec<MetricValue>,
    pub retention_days: u64,
    pub rpms: Vec<usize>,
    pub sampled_at: u64,
    pub slo_target_percent: f64,
    /// Unix time this process started.
    pub started: u64,
    /// Counters accumulated since the last persisted sample.
    pub tail: Tail,
    pub version: String,
}

// ---------------------------------------------------------------------------
// The generated spec
// ---------------------------------------------------------------------------

/// Auth schemes: a session cookie for browsers, HTTP Basic (or
/// `Bearer user:password`) for Prometheus scrapers. Either satisfies the
/// `require_session` guard, so they are listed as alternatives.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi
            .components
            .as_mut()
            .expect("components exist: every path declares a response schema");
        components.add_security_scheme(
            "session_cookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "nimproxy_session",
                "HMAC-signed session cookie minted by POST /login. The signing key is \
                 regenerated every boot, so a restart invalidates all sessions.",
            ))),
        );
        components.add_security_scheme(
            "basic_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Basic)
                    .description(Some(
                        "Dashboard username and password, for non-browser clients such as a \
                         Prometheus scraper. `Authorization: Bearer <user>:<password>` is \
                         accepted equivalently.",
                    ))
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "nim-proxy dashboard API",
        description = "The operator surface of nim-proxy: the settings store, user and key \
                       management, and the metrics history the dashboard renders. This document \
                       is generated from the handlers; regenerate it with \
                       `UPDATE_OPENAPI=1 cargo test --test openapi`.\n\n\
                       Not covered: the OpenAI-compatible `/v1` passthrough (that contract is \
                       the upstream's, not nim-proxy's), the HTML page routes, the form-encoded \
                       `/login` and `/logout` browser flow, plain-text `/health`, and the \
                       Prometheus exposition at `/metrics`.",
        license(name = "MIT", url = "https://github.com/miztertea/nim-proxy/blob/main/LICENSE"),
        contact(name = "nim-proxy", url = "https://github.com/miztertea/nim-proxy"),
    ),
    paths(
        crate::api_dashboard,
        crate::api_dashboard_now,
        crate::settings::api_config,
        crate::settings::nim_keys,
        crate::settings::clients,
        crate::settings::upstream,
        crate::settings::limits,
        crate::settings::history,
        crate::settings::governor_cfg,
        crate::settings::users,
        crate::settings::account,
        crate::settings::validate_key,
        crate::settings::setup_submit,
        crate::settings::setup_validate_key,
    ),
    components(schemas(
        ApiError,
        ApiErrorBody,
        ClientKeyRow,
        ClientsResponse,
        ConfigResponse,
        DashboardCfg,
        DashboardNowResponse,
        DashboardResponse,
        DashboardWindow,
        GovernorCfg,
        HistoryDiagnostics,
        HistorySettings,
        Limits,
        MetricValue,
        MintedClientKey,
        Mode,
        NimKeyRow,
        OkResponse,
        PoolSummary,
        Role,
        RollupPoint,
        ServerSettings,
        SetupResponse,
        Tail,
        UserRow,
        ValidateKeyResponse,
    )),
    // Applies to every operation that does not override it. The two `/setup`
    // routes declare `security()` — an explicit empty list, i.e. "no auth" —
    // because they run before any user exists.
    security(
        ("session_cookie" = []),
        ("basic_auth" = []),
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "dashboard", description = "Read-only telemetry the operator console renders."),
        (name = "settings", description = "The config store's only writers. Every write is \
            validate -> persist -> swap; a failed disk write applies nothing."),
        (name = "setup", description = "First-run claim. **Unauthenticated** — these routes sit \
            outside the session guard because no user exists yet. They answer 404 the moment a \
            superuser exists, so the window is exactly one claim wide."),
    ),
)]
pub struct ApiDoc;

/// The committed spec, pretty-printed with a trailing newline. `openapi.json`
/// at the repo root is exactly this string; CI regenerates and diffs.
pub fn openapi_json() -> String {
    let mut s = ApiDoc::openapi()
        .to_pretty_json()
        .expect("the generated spec serializes");
    s.push('\n');
    s
}

/// Serialized key order for a value, top level only.
#[cfg(test)]
fn keys_of<T: Serialize>(value: &T) -> Vec<String> {
    match serde_json::to_value(value).unwrap() {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        other => panic!("expected an object, got {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Declaration order is the wire order, and it must stay ASCII-sorted:
    /// that is what the `serde_json::Map` (a `BTreeMap`) behind the old
    /// `json!` bodies produced, so reordering a field silently reshapes a
    /// response that shipped. Serializing a fully-populated value and
    /// asserting the keys come out sorted catches it for every type at once.
    #[test]
    fn field_order_stays_ascii_sorted() {
        fn sorted<T: Serialize>(label: &str, value: &T) {
            let keys = keys_of(value);
            let mut want = keys.clone();
            want.sort();
            assert_eq!(
                keys, want,
                "{label}: field declaration order must be ASCII-sorted — it is the wire order"
            );
            assert!(!keys.is_empty(), "{label}: nothing was serialized");
        }

        sorted("ApiError", &ApiError::new("code", "message"));
        sorted(
            "CapacityRollup",
            &crate::history::CapacityRollup {
                average_rpm: 0.0,
                latest_rpms: vec![],
            },
        );
        sorted("ApiErrorBody", &ApiError::new("code", "message").error);
        sorted("OkResponse", &OkResponse::new());
        sorted(
            "ClientsResponse",
            &ClientsResponse {
                ok: true,
                secret: Some("npk_x".into()),
            },
        );
        sorted(
            "ValidateKeyResponse",
            &ValidateKeyResponse {
                error: Some("nope".into()),
                models: Some(1),
                ok: false,
            },
        );
        sorted(
            "SetupResponse",
            &SetupResponse {
                client_key: Some(MintedClientKey {
                    name: "first".into(),
                    secret: "npk_x".into(),
                }),
                ok: true,
            },
        );
        sorted(
            "MintedClientKey",
            &MintedClientKey {
                name: "first".into(),
                secret: "npk_x".into(),
            },
        );
        sorted(
            "ClientKeyRow",
            &ClientKeyRow {
                last4: "abcd".into(),
                name: "harness".into(),
                owner: "root".into(),
            },
        );
        sorted(
            "NimKeyRow",
            &NimKeyRow {
                cooldown_ms: Some(0),
                enabled: true,
                fingerprint: "abcd1234".into(),
                guarded: false,
                in_window: Some(0),
                lane: Some(0),
                last4: "wxyz".into(),
                owner: "root".into(),
                rpm: 40,
            },
        );
        sorted(
            "PoolSummary",
            &PoolSummary {
                capacity_rpm: 40,
                enabled: 1,
            },
        );
        sorted(
            "HistorySettings",
            &HistorySettings {
                available_from: Some(0),
                compaction_pending: false,
                days: 30,
                file_bytes: 0,
            },
        );
        sorted(
            "UserRow",
            &UserRow {
                client_keys: 0,
                nim_keys: 0,
                role: Role::User,
                username: "root".into(),
            },
        );
        let server = ServerSettings {
            base_url: "https://example.invalid".into(),
            dashboard: DashboardCfg::default(),
            governor: GovernorCfg::default(),
            history: HistorySettings {
                available_from: None,
                compaction_pending: false,
                days: 30,
                file_bytes: 0,
            },
            limits: Limits::default(),
        };
        sorted("Limits", &server.limits);
        sorted("DashboardCfg", &server.dashboard);
        sorted("GovernorCfg", &server.governor);
        sorted("ServerSettings", &server);
        sorted(
            "ConfigResponse",
            &ConfigResponse {
                client_keys: Vec::new(),
                mode: Mode::Keyed,
                nim_keys: Vec::new(),
                pool: PoolSummary {
                    capacity_rpm: 40,
                    enabled: 1,
                },
                role: Role::Superuser,
                server: Some(server),
                username: "root".into(),
                users: Some(Vec::new()),
            },
        );
        let window = DashboardWindow {
            available_from: None,
            available_to: None,
            default_window_days: 30,
            effective_from: None,
            effective_to: None,
            following_now: true,
            requested_from: 0,
            requested_to: 1,
            retention_days: 30,
        };
        sorted("DashboardWindow", &window);
        sorted(
            "DashboardResponse",
            &DashboardResponse {
                config_revision: 1,
                diagnostics: HistoryDiagnostics::default(),
                history_revision: 1,
                latest: Vec::new(),
                points: Vec::new(),
                totals: Vec::new(),
                window,
            },
        );
        sorted("HistoryDiagnostics", &HistoryDiagnostics::default());
        let tail = Tail {
            base_history_revision: 1,
            from: None,
            to: 1,
            totals: Vec::new(),
        };
        sorted("Tail", &tail);
        sorted(
            "DashboardNowResponse",
            &DashboardNowResponse {
                auth: true,
                available_from: None,
                available_to: None,
                capacity_rpm: 40,
                config_revision: 1,
                default_window_days: 30,
                history_revision: 1,
                lanes: 1,
                metrics: Vec::new(),
                retention_days: 30,
                rpms: vec![40],
                sampled_at: 0,
                slo_target_percent: 99.9,
                started: 0,
                tail,
                version: "0.0.0".into(),
            },
        );
        sorted(
            "MetricValue",
            &MetricValue {
                labels: BTreeMap::new(),
                metric: "nimproxy_requests_total".into(),
                value: 1.0,
            },
        );
        sorted(
            "RollupPoint",
            &RollupPoint {
                capacity: None,
                duration_seconds: 1,
                from: 0,
                to: 1,
                values: Vec::new(),
            },
        );
    }

    /// Absent, not null: a non-admin `/api/config` body must not even carry
    /// the admin keys, and a successful probe must not carry `error`.
    #[test]
    fn optional_sections_are_omitted_not_nulled() {
        let user_view = ConfigResponse {
            client_keys: Vec::new(),
            mode: Mode::Open,
            nim_keys: Vec::new(),
            pool: PoolSummary {
                capacity_rpm: 40,
                enabled: 1,
            },
            role: Role::User,
            server: None,
            username: "alice".into(),
            users: None,
        };
        assert_eq!(
            serde_json::to_string(&user_view).unwrap(),
            r#"{"client_keys":[],"mode":"open","nim_keys":[],"pool":{"capacity_rpm":40,"enabled":1},"role":"user","username":"alice"}"#
        );
        assert_eq!(
            serde_json::to_string(&ValidateKeyResponse::probed(Ok(3))).unwrap(),
            r#"{"models":3,"ok":true}"#
        );
        assert_eq!(
            serde_json::to_string(&ValidateKeyResponse::probed(Err("nope".into()))).unwrap(),
            r#"{"error":"nope","ok":false}"#
        );
        assert_eq!(
            serde_json::to_string(&SetupResponse {
                client_key: None,
                ok: true
            })
            .unwrap(),
            r#"{"ok":true}"#
        );
    }

    /// A null lane is meaningful (the key holds none), so those stay null.
    #[test]
    fn lane_state_is_null_for_a_benched_key() {
        assert_eq!(
            serde_json::to_string(&NimKeyRow {
                cooldown_ms: None,
                enabled: false,
                fingerprint: "abcd1234".into(),
                guarded: false,
                in_window: None,
                lane: None,
                last4: "wxyz".into(),
                owner: "root".into(),
                rpm: 40,
            })
            .unwrap(),
            r#"{"cooldown_ms":null,"enabled":false,"fingerprint":"abcd1234","guarded":false,"in_window":null,"lane":null,"last4":"wxyz","owner":"root","rpm":40}"#
        );
    }

    /// The error envelope is the `/v1` envelope: `{error:{code,message,type}}`.
    #[test]
    fn error_envelope_matches_the_v1_shape() {
        assert_eq!(
            serde_json::to_string(&ApiError::new(
                "forbidden",
                "server settings require an admin"
            ))
            .unwrap(),
            r#"{"error":{"code":"forbidden","message":"server settings require an admin","type":"proxy_error"}}"#
        );
    }

    #[test]
    fn api_error_wire_order_is_exact() {
        let body = serde_json::to_string(&ApiError::new("invalid_json", "invalid JSON")).unwrap();
        assert_eq!(
            body,
            r#"{"error":{"code":"invalid_json","message":"invalid JSON","type":"proxy_error"}}"#
        );
    }
}
