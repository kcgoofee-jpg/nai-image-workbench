use crate::proxy::{allow_subscription, stub_subscription};
use crate::state::AppState;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_SETTINGS: &str = r#"{"defaultModel":"nai-diffusion-4-5-curated","uiLanguage":"zh-CN","persistImageGenHistory":true}"#;
const STORE_LIMIT: usize = 1024 * 1024;

pub struct StudioDb {
    conn: Mutex<Connection>,
}

impl StudioDb {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            tighten(parent);
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS studio (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                keystore TEXT,
                keystore_change_index INTEGER NOT NULL DEFAULT 0,
                settings TEXT
            );
            INSERT OR IGNORE INTO studio (id) VALUES (1);",
        )
        .map_err(|e| e.to_string())?;
        tighten(path);
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn keystore(&self) -> Result<(Value, i64), String> {
        let conn = self.conn.lock().map_err(|_| "database busy".to_string())?;
        let row: Option<(Option<String>, i64)> = conn
            .query_row(
                "SELECT keystore, keystore_change_index FROM studio WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((blob, index)) = row else {
            return Ok((Value::Null, 0));
        };
        let value = blob
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or(Value::Null);
        Ok((value, index))
    }

    fn set_keystore(&self, value: &Value, index: i64) -> Result<(), String> {
        let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
        if text.len() > STORE_LIMIT {
            return Err("too large".into());
        }
        let conn = self.conn.lock().map_err(|_| "database busy".to_string())?;
        conn.execute(
            "UPDATE studio SET keystore = ?1, keystore_change_index = ?2 WHERE id = 1",
            (text, index),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn settings(&self) -> Result<Value, String> {
        let conn = self.conn.lock().map_err(|_| "database busy".to_string())?;
        let raw: Option<String> = conn
            .query_row("SELECT settings FROM studio WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        let mut value = match raw {
            Some(text) => serde_json::from_str(&text).unwrap_or_else(|_| json!({})),
            None => serde_json::from_str(DEFAULT_SETTINGS).unwrap(),
        };
        fill_settings(&mut value);
        Ok(value)
    }

    fn set_settings(&self, value: &Value) -> Result<(), String> {
        let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
        if text.len() > STORE_LIMIT {
            return Err("too large".into());
        }
        let conn = self.conn.lock().map_err(|_| "database busy".to_string())?;
        conn.execute("UPDATE studio SET settings = ?1 WHERE id = 1", [text])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn tighten(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
}

fn fill_settings(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    obj.entry("uiLanguage".to_string())
        .or_insert(json!("zh-CN"));
    obj.entry("persistImageGenHistory".to_string())
        .or_insert(json!(true));
    obj.entry("defaultModel".to_string())
        .or_insert(json!("nai-diffusion-4-5-curated"));
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn subscription(state: &AppState) -> Value {
    if let Some(cached) = fresh_cache(&state.subscription) {
        return cached;
    }
    let url = format!("{}/user/subscription", state.api_host.trim_end_matches('/'));
    let fetched = state
        .http
        .get(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", state.nai_token))
        .send()
        .await
        .ok()
        .filter(|res| res.status().is_success());
    let value = match fetched {
        Some(res) => match res.json::<Value>().await {
            Ok(raw) => allow_subscription(&raw, now_secs()),
            Err(_) => stub_subscription(now_secs()),
        },
        None => stub_subscription(now_secs()),
    };
    if let Ok(mut slot) = state.subscription.lock() {
        *slot = Some((Instant::now(), value.clone()));
    }
    value
}

fn fresh_cache(slot: &Mutex<Option<(Instant, Value)>>) -> Option<Value> {
    let guard = slot.lock().ok()?;
    let (at, value) = guard.as_ref()?;
    if at.elapsed() < Duration::from_secs(300) {
        Some(value.clone())
    } else {
        None
    }
}

pub async fn user_data(State(state): State<AppState>) -> Response {
    let sub = subscription(&state).await;
    let (keystore, change_index) = match state.db.keystore() {
        Ok(row) => row,
        Err(_) => {
            return crate::http_util::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Storage unavailable",
            )
        }
    };
    let settings = state
        .db
        .settings()
        .unwrap_or_else(|_| serde_json::from_str(DEFAULT_SETTINGS).unwrap());
    let keystore = if keystore.is_null() {
        Value::Null
    } else {
        json!({"keystore": keystore, "changeIndex": change_index})
    };
    crate::http_util::ok(json!({
        "subscription": sub,
        "priority": priority(),
        "trainingStepsLeft": sub["trainingStepsLeft"].clone(),
        "anlas": sub["trainingStepsLeft"]["fixedTrainingStepsLeft"].clone(),
        "v5Access": {"allowed": true, "paused": false},
        "keystore": keystore,
        "settings": serde_json::to_string(&settings).unwrap_or_else(|_| DEFAULT_SETTINGS.to_string()),
    }))
}

pub async fn user_subscription(State(state): State<AppState>) -> Response {
    crate::http_util::ok(subscription(&state).await)
}

pub async fn user_priority() -> Response {
    crate::http_util::ok(priority())
}

fn priority() -> Value {
    let now = now_secs();
    json!({
        "maxPriorityActions": 0,
        "nextRefillAt": now + (3600 - now.rem_euclid(3600)),
        "taskPriority": 10
    })
}

pub async fn user_keystore(State(state): State<AppState>) -> Response {
    match state.db.keystore() {
        Ok((value, index)) => {
            crate::http_util::ok(json!({"keystore": value, "changeIndex": index}))
        }
        Err(_) => crate::http_util::error(StatusCode::INTERNAL_SERVER_ERROR, "Storage unavailable"),
    }
}

#[derive(Deserialize)]
pub struct KeystoreBody {
    #[serde(default)]
    keystore: Option<Value>,
    #[serde(default, rename = "changeIndex")]
    change_index: Option<i64>,
}

pub async fn user_keystore_put(State(state): State<AppState>, req: Request) -> Response {
    let bytes = match axum::body::to_bytes(req.into_body(), STORE_LIMIT).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return crate::http_util::error(StatusCode::PAYLOAD_TOO_LARGE, "Keystore too large")
        }
    };
    let parsed: KeystoreBody = match serde_json::from_slice(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => return crate::http_util::error(StatusCode::BAD_REQUEST, "Invalid keystore"),
    };
    if state
        .db
        .set_keystore(
            &parsed.keystore.unwrap_or(Value::Null),
            parsed.change_index.unwrap_or(0),
        )
        .is_err()
    {
        return crate::http_util::error(StatusCode::BAD_REQUEST, "Keystore rejected");
    }
    crate::http_util::ok(json!({"ok": true}))
}

pub async fn user_settings(State(state): State<AppState>) -> Response {
    match state.db.settings() {
        Ok(value) => crate::http_util::ok(value),
        Err(_) => crate::http_util::error(StatusCode::INTERNAL_SERVER_ERROR, "Storage unavailable"),
    }
}

pub async fn user_settings_put(State(state): State<AppState>, req: Request) -> Response {
    let bytes = match axum::body::to_bytes(req.into_body(), STORE_LIMIT).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return crate::http_util::error(StatusCode::PAYLOAD_TOO_LARGE, "Settings too large")
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return crate::http_util::error(StatusCode::BAD_REQUEST, "Invalid settings"),
    };
    if !value.is_object() {
        return crate::http_util::error(StatusCode::BAD_REQUEST, "Invalid settings");
    }
    if state.db.set_settings(&value).is_err() {
        return crate::http_util::error(StatusCode::BAD_REQUEST, "Settings rejected");
    }
    crate::http_util::ok(json!({"ok": true}))
}

#[derive(Deserialize)]
pub struct LoginBody {
    #[serde(default)]
    key: Option<String>,
    #[serde(default, rename = "accessKey")]
    access_key: Option<String>,
}

pub async fn user_login(State(state): State<AppState>, req: Request) -> Response {
    let bytes = match axum::body::to_bytes(req.into_body(), 64 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return crate::http_util::error(StatusCode::PAYLOAD_TOO_LARGE, "Login body too large")
        }
    };
    let parsed: LoginBody = match serde_json::from_slice(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => return crate::http_util::error(StatusCode::BAD_REQUEST, "Invalid login"),
    };
    let token = parsed.key.or(parsed.access_key).unwrap_or_default();
    if !crate::auth::keys_match(token.trim(), &state.access_key) {
        state.record_failure();
        return crate::http_util::error(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    crate::http_util::ok(json!({"accessToken": "local"}))
}

pub async fn user_information() -> Response {
    let now = now_secs();
    crate::http_util::ok(json!({
        "tier": 3,
        "active": true,
        "username": "local",
        "expiresAt": now + 3650 * 86400
    }))
}

pub async fn empty_object() -> Response {
    crate::http_util::ok(json!({}))
}

pub async fn empty_list() -> Response {
    crate::http_util::ok(json!([]))
}

pub async fn empty_objects() -> Response {
    crate::http_util::ok(json!({"objects": []}))
}

pub async fn trial_status() -> Response {
    crate::http_util::ok(json!({
        "remaining_image_actions": 0,
        "remaining_text_actions": 0
    }))
}

pub async fn user_inventory() -> Response {
    crate::http_util::ok(json!({"items": []}))
}

#[derive(Deserialize)]
pub struct TokenizerQuery {
    sha256: String,
}

struct TokenizerSpec {
    file: &'static str,
    sha256: &'static str,
}

fn tokenizer_spec(model: &str) -> Option<TokenizerSpec> {
    match model {
        "qwen" => Some(TokenizerSpec {
            file: "qwen35_tokenizer.def",
            sha256: "f4040d875827d2f9edc30dd4d736bc4a853a19ceeb0d04f6e3648f17796d1667",
        }),
        "t5" => Some(TokenizerSpec {
            file: "t5_tokenizer.def",
            sha256: "3cbf698ff2c19efb5c75165d781c1d1b9c035baf119c3e71f1a380cbbfb9dfcb",
        }),
        "clip" => Some(TokenizerSpec {
            file: "clip_tokenizer.def",
            sha256: "05852800e8ca59374d5b6d6d3993d9ccea63b7613dc827d12abff64f601a1744",
        }),
        _ => None,
    }
}

fn tokenizer_cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn public_tokenizer(
    State(state): State<AppState>,
    Path(model): Path<String>,
    Query(query): Query<TokenizerQuery>,
) -> Response {
    let Some(spec) = tokenizer_spec(&model) else {
        return crate::http_util::error(StatusCode::NOT_FOUND, "Not Found");
    };
    if !crate::auth::keys_match(&query.sha256, spec.sha256) {
        return crate::http_util::error(StatusCode::CONFLICT, "Tokenizer hash mismatch");
    }
    if let Some(hit) = tokenizer_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&model).cloned())
    {
        return tokenizer_response(hit);
    }
    let url = format!(
        "https://novelai.net/tokenizer/compressed/{}?v=2&static=true",
        spec.file
    );
    let Ok(response) = state.http.get(url).send().await else {
        return crate::http_util::error(StatusCode::BAD_GATEWAY, "Tokenizer unavailable");
    };
    if !response.status().is_success() {
        return crate::http_util::error(StatusCode::BAD_GATEWAY, "Tokenizer unavailable");
    }
    let Ok(bytes) = response.bytes().await else {
        return crate::http_util::error(StatusCode::BAD_GATEWAY, "Tokenizer unavailable");
    };
    if bytes.len() > 8 * 1024 * 1024 {
        return crate::http_util::error(StatusCode::BAD_GATEWAY, "Tokenizer unavailable");
    }
    let digest = Sha256::digest(&bytes);
    let actual = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if !crate::auth::keys_match(&actual, spec.sha256) {
        return crate::http_util::error(StatusCode::BAD_GATEWAY, "Tokenizer unavailable");
    }
    if let Ok(mut cache) = tokenizer_cache().lock() {
        cache.insert(model, bytes.to_vec());
    }
    tokenizer_response(bytes.to_vec())
}

fn tokenizer_response(bytes: Vec<u8>) -> Response {
    let mut response =
        ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    response
}
