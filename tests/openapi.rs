//! `openapi.json` is generated from the handlers, so it can only be right by
//! construction — this test is what makes it *stay* right. It regenerates the
//! spec and compares it to the committed file; drift fails the build.
//!
//! Regenerate after changing a handler, a response type, or the crate
//! version:
//!
//! ```sh
//! UPDATE_OPENAPI=1 cargo test --test openapi
//! ```
//!
//! CI runs exactly that and then `git diff --exit-code -- openapi.json`, so a
//! PR cannot merge a spec that disagrees with the code.

use std::path::PathBuf;

fn assert_global_security(spec: &serde_json::Value) {
    let global = spec["security"].as_array().expect("document security");
    assert_eq!(
        global,
        &[
            serde_json::json!({"session_cookie": []}),
            serde_json::json!({"basic_auth": []}),
        ],
        "route-contract:openapi-security: the /api/* routes inherit exactly session-cookie or Basic credentials"
    );
}

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi.json")
}

#[test]
fn committed_spec_matches_the_code() {
    let generated = nim_proxy::openapi_json();
    let path = spec_path();

    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(&path, &generated).expect("write openapi.json");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nRegenerate it with `UPDATE_OPENAPI=1 cargo test --test openapi`",
            path.display()
        )
    });
    assert_eq!(
        committed, generated,
        "openapi.json is out of date with the handlers.\n\
         Regenerate it with `UPDATE_OPENAPI=1 cargo test --test openapi` and commit the result."
    );
}

/// A spec nothing can consume is worse than none: assert the invariants a
/// generator client actually needs, rather than trusting the file blindly.
#[test]
fn spec_is_usable() {
    let spec: serde_json::Value =
        serde_json::from_str(&nim_proxy::openapi_json()).expect("the spec is JSON");

    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["version"], env!("CARGO_PKG_VERSION"));

    let paths = spec["paths"].as_object().expect("paths");
    assert_eq!(paths.len(), 15, "13 /api/* routes + the 2 setup routes");
    assert_eq!(
        paths["/api/locale-bootstrap"]["get"]["security"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "GET /api/locale-bootstrap is intentionally public"
    );
    for omitted in [
        "/",
        "/dash",
        "/health",
        "/login",
        "/logout",
        "/metrics",
        "/v1/{*path}",
    ] {
        assert!(
            !paths.contains_key(omitted),
            "{omitted}: non-control-plane or upstream-owned route must stay outside this spec"
        );
    }

    for (path, item) in paths {
        for (method, op) in item.as_object().expect("path item") {
            let label = format!("{method} {path}");
            assert!(
                op["responses"]["200"].is_object(),
                "{label}: no documented success response"
            );
            assert!(
                op["tags"].as_array().is_some_and(|t| !t.is_empty()),
                "{label}: untagged"
            );
            // The setup routes sit outside the session guard and say so with
            // an explicit empty `security` list; /api/* routes carry none of
            // their own and so inherit the document-level requirement.
            if path == "/api/locale-bootstrap" {
                assert_eq!(
                    op["security"].as_array().map(Vec::len),
                    Some(0),
                    "{label}: locale bootstrap is public by design"
                );
            } else if path.starts_with("/api/") {
                assert!(
                    op.get("security").is_none(),
                    "{label}: an /api/* route must inherit the document security requirement"
                );
            } else {
                assert_eq!(
                    op["security"].as_array().map(Vec::len),
                    Some(0),
                    "{label}: the setup routes are unauthenticated by design and must say so"
                );
            }
        }
    }

    for (path, item) in paths {
        for (method, op) in item.as_object().expect("path item") {
            for (status, response) in op["responses"].as_object().expect("responses") {
                if status.starts_with('2') {
                    continue;
                }
                assert_eq!(
                    response["content"]["application/json"]["schema"]["$ref"],
                    "#/components/schemas/ApiError",
                    "{method} {path} {status}: every JSON API rejection uses ApiError"
                );
            }
        }
    }

    // The document-level requirement the /api/* routes inherit.
    assert_global_security(&spec);

    // Every schema a response references must actually be in components.
    let schemas = spec["components"]["schemas"]
        .as_object()
        .expect("components.schemas");
    for name in [
        "ApiError",
        "ConfigResponse",
        "DashboardResponse",
        "LocaleBootstrap",
        "OkResponse",
    ] {
        assert!(schemas.contains_key(name), "{name} is missing");
    }
    let security = spec["components"]["securitySchemes"]
        .as_object()
        .expect("securitySchemes");
    assert_eq!(security["session_cookie"]["type"], "apiKey");
    assert_eq!(security["session_cookie"]["in"], "cookie");
    assert_eq!(security["session_cookie"]["name"], "nimproxy_session");
    assert_eq!(security["basic_auth"]["type"], "http");
    assert_eq!(security["basic_auth"]["scheme"], "basic");
}

#[test]
fn locale_bootstrap_schema_is_typed() {
    let spec: serde_json::Value =
        serde_json::from_str(&nim_proxy::openapi_json()).expect("the spec is JSON");
    assert_eq!(
        spec["paths"]["/api/locale-bootstrap"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/LocaleBootstrap",
        "locale bootstrap success must reference its Rust response type"
    );
    let schema = &spec["components"]["schemas"]["LocaleBootstrap"];
    assert_eq!(schema["type"], "object");
    assert_eq!(
        schema["required"],
        serde_json::json!(["installed_locales", "server_default"])
    );
    assert_eq!(schema["properties"]["installed_locales"]["type"], "array");
    assert_eq!(
        schema["properties"]["installed_locales"]["items"]["type"],
        "string"
    );
    assert_eq!(schema["properties"]["server_default"]["type"], "string");
}

#[test]
#[should_panic(expected = "route-contract:openapi-security")]
fn global_security_self_test_names_wrong_requirement() {
    let mut spec: serde_json::Value =
        serde_json::from_str(&nim_proxy::openapi_json()).expect("generated OpenAPI");
    spec["security"] = serde_json::json!([{"wrong_scheme": []}]);
    assert_global_security(&spec);
}
