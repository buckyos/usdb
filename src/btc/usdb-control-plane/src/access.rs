//! Private-console authentication and browser origin boundary.

use axum::{
    Json,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const COOKIE: &str = "usdb_console_session";
const LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

/// Shared access policy; secrets are never included in diagnostics or responses.
pub struct Access {
    token: String,
    origins: Vec<String>,
    development: bool,
    sessions: Mutex<HashMap<String, Instant>>,
}

fn random_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Cannot generate console access secret".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Create a persistent, owner-readable access token on first startup.
pub fn load_token(root: &Path) -> Result<String, String> {
    let path = root.join("access-token");
    if !path.exists() {
        let token = random_secret()?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => file
                .write_all(format!("{token}\n").as_bytes())
                .map_err(|_| "Cannot persist console access token".to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("Cannot create console access-token file".into()),
        }
    }
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| "Cannot inspect console access-token file")?;
    if !metadata.is_file() || metadata.len() > 128 {
        return Err("Invalid console access-token file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Console access-token permissions must be 0600".into());
        }
    }
    let token = std::fs::read_to_string(&path).map_err(|_| "Cannot read console access token")?;
    let token = token.trim();
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid console access token; expected 64 hexadecimal characters".into());
    }
    Ok(token.to_owned())
}

impl Access {
    pub fn new(token: String, origins: Vec<String>, development: bool) -> Result<Self, String> {
        for origin in &origins {
            let url = reqwest::Url::parse(origin).map_err(|_| "Invalid console allowed origin")?;
            if !matches!(url.scheme(), "http" | "https")
                || url.origin().ascii_serialization() != *origin
            {
                return Err(
                    "Console allowed origins must be HTTP(S) origins without a path".into(),
                );
            }
        }
        Ok(Self {
            token,
            origins,
            development,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    fn allowed_host(&self, host: &str) -> bool {
        let Ok(url) = reqwest::Url::parse(&format!("http://{host}")) else {
            return false;
        };
        if url.username() != ""
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return false;
        }
        if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
            return true;
        }
        self.origins.iter().any(|origin| {
            origin
                .split_once("://")
                .is_some_and(|(_, authority)| authority == host)
        })
    }

    fn authenticated(&self, request: &Request) -> bool {
        if let Some(value) = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            return equal_secret(&self.token, value);
        }
        session_cookie(request).is_some_and(|cookie| {
            self.sessions
                .lock()
                .expect("session lock")
                .get(cookie)
                .is_some_and(|created| created.elapsed() < LIFETIME)
        })
    }

    fn allowed_origin(&self, origin: &str, host: &str) -> bool {
        if origin != format!("http://{host}") && origin != format!("https://{host}") {
            return false;
        }
        let loopback = reqwest::Url::parse(origin)
            .ok()
            .is_some_and(|url| matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")));
        loopback || self.origins.iter().any(|allowed| allowed == origin)
    }
}

fn equal_secret(expected: &str, received: &str) -> bool {
    expected.len() == received.len()
        && expected
            .bytes()
            .zip(received.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn session_cookie(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| part.trim().strip_prefix(&format!("{COOKIE}=")))
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

/// Enforce Host/Origin checks before any API, including development routes.
pub async fn guard(State(access): State<Arc<Access>>, request: Request, next: Next) -> Response {
    let mut response = dispatch(access, request, next).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    response
}

async fn dispatch(access: Arc<Access>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !access.allowed_host(host) {
        return error(
            StatusCode::FORBIDDEN,
            "Console Host is not allowed; use localhost through an SSH tunnel",
        );
    }
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if origin.is_some_and(|origin| !access.allowed_origin(origin, host))
        || request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site")
    {
        return error(
            StatusCode::FORBIDDEN,
            "Cross-origin console access is not allowed",
        );
    }
    let path = request.uri().path().to_owned();
    if path == "/api/auth/login" && request.method() == Method::POST {
        if origin.is_none()
            || !request
                .headers()
                .get(header::CONTENT_TYPE)
                .is_some_and(|v| v.to_str().unwrap_or("").starts_with("application/json"))
        {
            return error(
                StatusCode::FORBIDDEN,
                "Login requires a same-origin JSON request",
            );
        }
        let secure = origin.is_some_and(|v| v.starts_with("https://"));
        let body = match to_bytes(request.into_body(), 4096).await {
            Ok(body) => body,
            Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid login request"),
        };
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        if !equal_secret(&access.token, value["token"].as_str().unwrap_or("")) {
            return error(StatusCode::UNAUTHORIZED, "Invalid console access token");
        }
        let session = match random_secret() {
            Ok(value) => value,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Cannot create console session",
                );
            }
        };
        let mut sessions = access.sessions.lock().expect("session lock");
        sessions.retain(|_, created| created.elapsed() < LIFETIME);
        if sessions.len() >= 64 {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many console sessions; sign out or restart the console",
            );
        }
        sessions.insert(session.clone(), Instant::now());
        let cookie = format!(
            "{COOKIE}={session}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200{}",
            if secure { "; Secure" } else { "" }
        );
        return (
            [(header::SET_COOKIE, cookie)],
            Json(json!({"authenticated": true})),
        )
            .into_response();
    }
    if path == "/api/auth/session" {
        return Json(json!({"authenticated": access.authenticated(&request)})).into_response();
    }
    if path.starts_with("/api/") && !access.authenticated(&request) {
        return error(StatusCode::UNAUTHORIZED, "Sign in to the private console");
    }
    if path == "/api/auth/logout" && request.method() == Method::POST {
        if let Some(cookie) = session_cookie(&request) {
            access.sessions.lock().expect("session lock").remove(cookie);
        }
        return (
            [(
                header::SET_COOKIE,
                format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
            )],
            Json(json!({"authenticated": false})),
        )
            .into_response();
    }
    if !access.development
        && (path.contains("/world-sim/")
            || path.contains("/dev-sim/")
            || path == "/api/btc/mint/execute")
    {
        return error(
            StatusCode::FORBIDDEN,
            "Development wallet operations are disabled",
        );
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, routing::get};
    use tower::ServiceExt;

    fn request(
        method: Method,
        path: &str,
        origin: Option<&str>,
        cookie: Option<&str>,
        body: &str,
    ) -> Request {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "localhost:28040")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        request.body(Body::from(body.to_owned())).unwrap()
    }

    #[tokio::test]
    async fn private_api_requires_login_rejects_cross_origin_and_revokes_logout() {
        let access = Arc::new(Access::new("a".repeat(64), vec![], false).unwrap());
        let app = Router::new()
            .route("/api/private", get(|| async { "private" }))
            .fallback(|| async { "fallback" })
            .layer(axum::middleware::from_fn_with_state(access, guard));
        let send = |request| app.clone().oneshot(request);
        let denied = send(request(Method::GET, "/api/private", None, None, ""))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
        let body = format!(r#"{{"token":"{}"}}"#, "a".repeat(64));
        for origin in [None, Some("http://evil.example")] {
            assert_eq!(
                send(request(
                    Method::POST,
                    "/api/auth/login",
                    origin,
                    None,
                    &body
                ))
                .await
                .unwrap()
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let origin = Some("http://localhost:28040");
        assert_eq!(
            send(request(
                Method::POST,
                "/api/auth/login",
                origin,
                None,
                r#"{"token":"wrong"}"#
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::UNAUTHORIZED
        );
        let login = send(request(
            Method::POST,
            "/api/auth/login",
            origin,
            None,
            &body,
        ))
        .await
        .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        assert!(!cookie.contains(&"a".repeat(64)));
        let cookie = Some(cookie.split(';').next().unwrap());
        assert_eq!(
            send(request(Method::GET, "/api/private", origin, cookie, ""))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            send(request(
                Method::GET,
                "/api/private",
                Some("http://evil.example"),
                cookie,
                ""
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(request(
                Method::GET,
                "/api/btc/world-sim/dev-signer",
                origin,
                cookie,
                ""
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(request(
                Method::POST,
                "/api/btc/mint/execute",
                origin,
                cookie,
                "{}"
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(request(
                Method::POST,
                "/api/auth/logout",
                origin,
                cookie,
                ""
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            send(request(Method::GET, "/api/private", origin, cookie, ""))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn private_host_boundary_rejects_rebinding_and_ambiguous_authorities() {
        let access = Access::new(
            "a".repeat(64),
            vec!["https://console.example".into()],
            false,
        )
        .unwrap();
        for host in [
            "localhost:28040",
            "127.0.0.1:28040",
            "[::1]:28040",
            "console.example",
        ] {
            assert!(access.allowed_host(host), "{host}");
        }
        for host in [
            "evil.example",
            "localhost.evil.example",
            "evil@localhost",
            "localhost/path",
            "localhost#bad",
        ] {
            assert!(!access.allowed_host(host), "{host}");
        }
        assert!(Access::new("a".repeat(64), vec!["http://localhost/path".into()], false).is_err());
    }
}
