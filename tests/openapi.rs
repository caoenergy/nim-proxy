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
    assert_eq!(paths.len(), 14, "12 /api/* routes + the 2 setup routes");

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
            if path.starts_with("/api/") {
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
    let global = spec["security"].as_array().expect("document security");
    assert_eq!(global.len(), 2, "session cookie or header credentials");

    // Every schema a response references must actually be in components.
    let schemas = spec["components"]["schemas"]
        .as_object()
        .expect("components.schemas");
    for name in [
        "ApiError",
        "ConfigResponse",
        "DashboardResponse",
        "OkResponse",
    ] {
        assert!(schemas.contains_key(name), "{name} is missing");
    }
    let security = spec["components"]["securitySchemes"]
        .as_object()
        .expect("securitySchemes");
    assert!(security.contains_key("session_cookie") && security.contains_key("basic_auth"));
}
