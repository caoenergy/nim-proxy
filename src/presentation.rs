//! Compile-time embedded presentation pages and same-origin assets.

pub enum Page {
    Dashboard,
    Login { error_code: Option<&'static str> },
    Setup,
}

pub struct Asset {
    pub body: &'static [u8],
    pub content_type: &'static str,
}

const DASHBOARD: &str = include_str!("web/dashboard.html");
const LOGIN: &str = include_str!("web/login.html");
const SETUP: &str = include_str!("web/setup.html");
const NIM_PROXY_ICON: &str = include_str!("web/icons/nim-proxy.svg");

pub fn page(page: Page) -> String {
    match page {
        Page::Dashboard => DASHBOARD.replace("<!-- nim-proxy-icon -->", NIM_PROXY_ICON),
        Page::Login { error_code } => {
            let code = match error_code {
                Some("invalid_credentials") => "invalid_credentials",
                _ => "",
            };
            LOGIN.replace("{{error_code}}", code)
        }
        Page::Setup => SETUP.to_owned(),
    }
}

pub fn public_asset(path: &str) -> Option<Asset> {
    match path {
        "/assets/public/public.css" => Some(Asset {
            body: include_bytes!("web/public.css"),
            content_type: "text/css; charset=utf-8",
        }),
        "/assets/public/setup.js" => Some(Asset {
            body: include_bytes!("web/setup.js"),
            content_type: "text/javascript; charset=utf-8",
        }),
        "/assets/public/login.js" => Some(Asset {
            body: include_bytes!("web/login.js"),
            content_type: "text/javascript; charset=utf-8",
        }),
        _ => None,
    }
}

pub fn operator_asset(path: &str) -> Option<Asset> {
    match path {
        "/assets/operator/operator.css" => Some(Asset {
            body: include_bytes!("web/operator.css"),
            content_type: "text/css; charset=utf-8",
        }),
        "/assets/operator/shared.js" => Some(Asset {
            body: include_bytes!("web/shared.js"),
            content_type: "text/javascript; charset=utf-8",
        }),
        "/assets/operator/dashboard.js" => Some(Asset {
            body: include_bytes!("web/dashboard.js"),
            content_type: "text/javascript; charset=utf-8",
        }),
        "/assets/operator/settings.js" => Some(Asset {
            body: include_bytes!("web/settings.js"),
            content_type: "text/javascript; charset=utf-8",
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_error_code_is_allowlisted() {
        let invalid = page(Page::Login {
            error_code: Some("invalid_credentials"),
        });
        assert!(invalid.contains(r#"data-error-code="invalid_credentials""#));

        let unknown = page(Page::Login {
            error_code: Some("<b>not trusted</b>"),
        });
        assert!(unknown.contains(r#"data-error-code="""#));
        assert!(!unknown.contains("<b>not trusted</b>"));
    }
}
