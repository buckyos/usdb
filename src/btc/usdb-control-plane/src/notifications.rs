//! Notification configuration only. The host owns delivery, event history and credentials in use.
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 65536;
const SCHEMA: &str = "usdb-notifications:v1";

/// A narrow configuration mount; never the host event or delivery database directory.
#[derive(Clone)]
pub struct Settings {
    root: Option<PathBuf>,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl Settings {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

fn defaults() -> Value {
    json!({"schema_version": SCHEMA, "warning_interval_secs": 1800, "critical_interval_secs": 300,
           "notify_recovery": true, "channels": []})
}

fn fail(code: StatusCode, message: &str) -> Response {
    warn!(
        "Notification configuration request failed: code={}, reason={}",
        code, message
    );
    (code, Json(json!({"error": message}))).into_response()
}

fn private_root(root: &Path) -> Result<fs::Metadata, &'static str> {
    let meta = fs::symlink_metadata(root)
        .map_err(|_| "Notification configuration directory unavailable")?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err("Notification configuration directory must be private (0700)");
    }
    Ok(meta)
}

fn read(root: &Path, name: &str) -> Result<Value, &'static str> {
    let owner = private_root(root)?.uid();
    let path = root.join(name);
    let meta =
        fs::symlink_metadata(&path).map_err(|_| "Notification configuration file unavailable")?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != owner
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_BYTES
    {
        return Err("Unsafe notification configuration file");
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "Cannot read notification configuration")?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read notification configuration")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Notification configuration exceeds 64 KiB");
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid notification JSON")
}

fn random_id() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Cannot create notification request ID")?;
    Ok(bytes.iter().map(|v| format!("{v:02x}")).collect())
}

fn write(root: &Path, name: &str, value: &Value, exclusive: bool) -> Result<(), &'static str> {
    let meta = private_root(root)?;
    let path = root.join(name);
    if fs::symlink_metadata(&path).is_ok() {
        if exclusive {
            return Err("A notification test is already pending");
        }
        // Malformed JSON can be repaired, but unsafe filesystem targets cannot.
        let old =
            fs::symlink_metadata(&path).map_err(|_| "Cannot inspect notification configuration")?;
        if !old.is_file() || old.nlink() != 1 || old.uid() != meta.uid() || old.mode() & 0o077 != 0
        {
            return Err("Unsafe notification configuration file");
        }
    }
    let temporary = root.join(format!(".notify-{}", random_id()?));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| "Cannot create notification configuration")?;
        let bytes =
            serde_json::to_vec(value).map_err(|_| "Cannot encode notification configuration")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Notification configuration exceeds 64 KiB");
        }
        file.write_all(&bytes)
            .map_err(|_| "Cannot write notification configuration")?;
        // Docker may run as root; preserve the host operator's ownership on every save.
        if file
            .metadata()
            .map_err(|_| "Cannot inspect notification configuration")?
            .uid()
            != meta.uid()
        {
            std::os::unix::fs::chown(&temporary, Some(meta.uid()), Some(meta.gid()))
                .map_err(|_| "Cannot preserve notification configuration ownership")?;
        }
        file.sync_all()
            .map_err(|_| "Cannot persist notification configuration")?;
        // Requests share the Settings mutex. Publish only complete bytes; a hard
        // link would briefly violate the host reader's single-link file policy.
        fs::rename(&temporary, &path).map_err(|_| "Cannot publish notification configuration")?;
        fs::File::open(root)
            .and_then(|file| file.sync_all())
            .map_err(|_| "Cannot persist notification directory")?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}

fn text(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
}

fn mailbox(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        let Some((local, domain)) = s.split_once('@') else {
            return false;
        };
        s.len() <= 254
            && !local.is_empty()
            && !domain.is_empty()
            && local
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&c))
            && domain
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
    })
}

/// Match the host's strict file schema; no endpoint or credential is echoed in errors.
fn validate(raw: Value) -> Result<Value, &'static str> {
    let mut result = defaults();
    let input = raw
        .as_object()
        .ok_or("Notification configuration must be an object")?;
    for (key, value) in input {
        if result.get(key).is_none() {
            return Err("Unknown notification configuration field");
        }
        result[key] = value.clone();
    }
    if result["schema_version"] != SCHEMA {
        return Err("Unsupported notification configuration version");
    }
    for key in ["warning_interval_secs", "critical_interval_secs"] {
        if !result[key]
            .as_u64()
            .is_some_and(|v| (60..=604800).contains(&v))
        {
            return Err("Notification intervals must be integers between 60 and 604800 seconds");
        }
    }
    if !result["notify_recovery"].is_boolean() {
        return Err("notify_recovery must be boolean");
    }
    let channels = result["channels"]
        .as_array_mut()
        .ok_or("channels must be an array")?;
    if channels.len() > 16 {
        return Err("At most 16 channels are supported");
    }
    let mut ids = HashSet::new();
    for raw_channel in channels {
        let mut channel = match raw_channel["type"].as_str() {
            Some("webhook") => {
                json!({"id":"", "type":"webhook", "enabled":true, "min_severity":"warning", "url":"", "bearer_token":"", "signing_secret":"", "allow_http":false})
            }
            Some("smtp") => {
                json!({"id":"", "type":"smtp", "enabled":true, "min_severity":"warning", "host":"", "port":587, "tls":"starttls", "username":"", "password":"", "sender":"", "recipients":[]})
            }
            _ => return Err("Channel type must be webhook or smtp"),
        };
        for (key, value) in raw_channel.as_object().ok_or("Channel must be an object")? {
            if channel.get(key).is_none() {
                return Err("Unknown channel field");
            }
            channel[key] = value.clone();
        }
        let id = channel["id"].as_str().ok_or("Channel ID required")?;
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            || !ids.insert(id.to_owned())
        {
            return Err("Channel IDs must be unique letters, digits, underscores or hyphens");
        }
        if !channel["enabled"].is_boolean()
            || !matches!(
                channel["min_severity"].as_str(),
                Some("warning" | "critical")
            )
        {
            return Err("Invalid channel enabled or min_severity");
        }
        for (key, value) in channel.as_object().unwrap() {
            if !["recipients", "port", "enabled", "allow_http"].contains(&key.as_str())
                && !text(value)
            {
                return Err("Channel fields must be bounded strings without control characters");
            }
        }
        if channel["type"] == "webhook" {
            if !channel["allow_http"].is_boolean() {
                return Err("allow_http must be boolean");
            }
            let url = reqwest::Url::parse(channel["url"].as_str().unwrap())
                .map_err(|_| "Invalid webhook URL")?;
            if (url.scheme() != "https"
                && !(url.scheme() == "http" && channel["allow_http"] == true))
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "Webhook requires HTTPS; HTTP requires allow_http; userinfo and fragments are forbidden",
                );
            }
        } else {
            let host = channel["host"].as_str().unwrap();
            if host.is_empty()
                || host.len() > 253
                || !host
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
                || !channel["port"]
                    .as_u64()
                    .is_some_and(|v| (1..=65535).contains(&v))
                || !matches!(channel["tls"].as_str(), Some("tls" | "starttls"))
            {
                return Err("Invalid SMTP host, port or TLS mode");
            }
            let recipients = channel["recipients"]
                .as_array()
                .ok_or("SMTP recipients must be an array")?;
            let mut addresses = HashSet::new();
            if recipients.is_empty()
                || recipients.len() > 16
                || !mailbox(&channel["sender"])
                || recipients
                    .iter()
                    .any(|v| !mailbox(v) || !addresses.insert(v.as_str()))
            {
                return Err(
                    "SMTP requires a sender and 1-16 distinct plain ASCII mailbox recipients",
                );
            }
        }
        *raw_channel = channel;
    }
    Ok(result)
}

fn redact(mut value: Value) -> Value {
    if let Some(channels) = value["channels"].as_array_mut() {
        for channel in channels {
            for key in ["url", "bearer_token", "signing_secret", "password"] {
                if channel.get(key).is_some() {
                    channel[format!("{key}_set")] =
                        json!(channel[key].as_str().is_some_and(|v| !v.is_empty()));
                    channel[key] = json!("");
                }
            }
        }
    }
    value
}

fn preserve_secrets(mut incoming: Value, old: &Value) -> Value {
    if let Some(channels) = incoming["channels"].as_array_mut() {
        for channel in channels {
            let previous = old["channels"].as_array().and_then(|items| {
                items
                    .iter()
                    .find(|v| v["id"] == channel["id"] && v["type"] == channel["type"])
            });
            for key in ["url", "bearer_token", "signing_secret", "password"] {
                let marker = format!("{key}_set");
                let retain = channel[&marker] == true;
                if let Some(fields) = channel.as_object_mut() {
                    fields.remove(&marker);
                }
                if retain
                    && channel.get(key).is_none_or(|v| v == "")
                    && let Some(value) = previous.and_then(|v| v.get(key))
                {
                    channel[key] = value.clone();
                }
            }
        }
    }
    incoming
}

/// Only these endpoints can write the dedicated notification configuration mount.
pub async fn get_config(State(state): State<Settings>) -> Response {
    let Some(root) = state.root else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Notification configuration is not mounted; update the node kit and services image",
        );
    };
    let _lock = state.lock.lock().await;
    match read(&root, "config.json").and_then(validate) {
        Ok(config) => Json(json!({"config": redact(config)})).into_response(),
        Err(message) => fail(StatusCode::BAD_REQUEST, message),
    }
}

pub async fn save_config(State(state): State<Settings>, Json(incoming): Json<Value>) -> Response {
    let Some(root) = state.root else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Notification configuration is not mounted",
        );
    };
    let _lock = state.lock.lock().await;
    let old = read(&root, "config.json").unwrap_or_else(|_| defaults());
    let value = match validate(preserve_secrets(incoming, &old)) {
        Ok(value) => value,
        Err(message) => return fail(StatusCode::BAD_REQUEST, message),
    };
    match write(&root, "config.json", &value, false) {
        Ok(()) => {
            info!(
                "Notification configuration saved: channels={}",
                value["channels"].as_array().unwrap().len()
            );
            Json(json!({"config": redact(value)})).into_response()
        }
        Err(message) => fail(StatusCode::BAD_REQUEST, message),
    }
}

pub async fn test_channel(State(state): State<Settings>, Json(request): Json<Value>) -> Response {
    let Some(root) = state.root else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Notification configuration is not mounted",
        );
    };
    let _lock = state.lock.lock().await;
    let config = match read(&root, "config.json").and_then(validate) {
        Ok(v) => v,
        Err(message) => return fail(StatusCode::BAD_REQUEST, message),
    };
    let Some(channel) = request["channel"].as_str() else {
        return fail(StatusCode::BAD_REQUEST, "Channel ID required");
    };
    if !config["channels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["id"] == channel && v["enabled"] == true)
    {
        return fail(
            StatusCode::BAD_REQUEST,
            "Save and enable this channel before testing",
        );
    }
    let identity = match random_id() {
        Ok(v) => v,
        Err(message) => return fail(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let request = json!({"id":identity, "channel":channel, "at_ms":at});
    match write(&root, "test-request.json", &request, true) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"request_id":identity, "state":"pending"})),
        )
            .into_response(),
        Err(message) => fail(StatusCode::CONFLICT, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::Request,
        routing::{get, post},
    };
    use tower::ServiceExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("usdb-notifications-{}", random_id().unwrap()));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn configured() -> Value {
        validate(json!({"channels": [{"id":"ops", "type":"webhook", "url":"https://example.invalid/private-token", "bearer_token":"private"}]})).unwrap()
    }

    #[test]
    fn masked_roundtrip_preserves_credentials_and_explicit_clear_removes_them() {
        let old = configured();
        let mut masked = redact(old.clone());
        assert!(!masked.to_string().contains("private"));
        masked["warning_interval_secs"] = json!(600);
        let saved = validate(preserve_secrets(masked.clone(), &old)).unwrap();
        assert_eq!(saved["channels"][0]["url"], old["channels"][0]["url"]);
        assert_eq!(saved["channels"][0]["bearer_token"], "private");
        masked["channels"][0]["bearer_token_set"] = json!(false);
        assert_eq!(
            validate(preserve_secrets(masked, &old)).unwrap()["channels"][0]["bearer_token"],
            ""
        );
    }

    #[test]
    fn invalid_inputs_never_echo_secrets() {
        for value in [
            json!({"warning_interval_secs":true}),
            json!({"channels":[{"id":"bad", "type":"webhook", "url":"http://private-secret.invalid"}]}),
            json!({"channels":[{"id":"mail", "type":"smtp", "host":"mail.invalid", "sender":"bad\nsecret", "recipients":["ops@example.invalid"]}]}),
        ] {
            let error = validate(value).unwrap_err();
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn private_atomic_files_and_unsafe_targets() {
        let fixture = Fixture::new();
        write(&fixture.0, "config.json", &configured(), false).unwrap();
        assert_eq!(read(&fixture.0, "config.json").unwrap(), configured());
        assert_eq!(
            fs::metadata(fixture.0.join("config.json")).unwrap().mode() & 0o777,
            0o600
        );
        let outside = fixture.0.join("outside");
        fs::write(&outside, "keep").unwrap();
        fs::remove_file(fixture.0.join("config.json")).unwrap();
        std::os::unix::fs::symlink(&outside, fixture.0.join("config.json")).unwrap();
        assert!(write(&fixture.0, "config.json", &configured(), false).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
    }

    #[tokio::test]
    async fn authenticated_configuration_and_test_requests_use_only_dedicated_files() {
        let fixture = Fixture::new();
        write(&fixture.0, "config.json", &defaults(), false).unwrap();
        let token = "a".repeat(64);
        let access = Arc::new(crate::access::Access::new(token.clone(), vec![], false).unwrap());
        let app = Router::new()
            .route(
                "/api/monitor/notifications/config",
                get(get_config).put(save_config),
            )
            .route("/api/monitor/notifications/test", post(test_channel))
            .with_state(Settings::new(Some(fixture.0.clone())))
            .layer(axum::middleware::from_fn_with_state(
                access,
                crate::access::guard,
            ));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/monitor/notifications/config")
                    .header("Host", "localhost:28040")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let call = |method: &str, path: &str, value: Value| {
            Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "localhost:28040")
                .header("Origin", "http://localhost:28040")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(call(
                "PUT",
                "/api/monitor/notifications/config",
                configured(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("private"));
        let response = app
            .clone()
            .oneshot(call(
                "POST",
                "/api/monitor/notifications/test",
                json!({"channel":"ops"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            read(&fixture.0, "test-request.json").unwrap()["channel"],
            "ops"
        );
        let response = app
            .oneshot(call(
                "POST",
                "/api/monitor/notifications/test",
                json!({"channel":"ops"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!fixture.0.join("events.sqlite3").exists());
    }
}
