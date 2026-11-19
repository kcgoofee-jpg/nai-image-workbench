use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};

const BODY_LIMIT: usize = 32 * 1024 * 1024;

pub fn image_upstream(method: &Method, path: &str) -> bool {
    let path = normalize(path);
    matches!(
        (method, path),
        (
            &Method::POST,
            "/ai/generate-image"
                | "/ai/generate-image-stream"
                | "/ai/encode-vibe"
                | "/ai/upscale"
                | "/ai/augment-image"
        ) | (&Method::GET, "/ai/generate-image/suggest-tags")
    )
}

fn normalize(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or(path);
    path.strip_prefix("/v1")
        .or_else(|| path.strip_prefix("/nai"))
        .unwrap_or(path)
}

pub async fn proxy_ai(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().map(str::to_string);
    if !image_upstream(&parts.method, &path) {
        return crate::http_util::error(StatusCode::NOT_FOUND, "Not Found");
    }
    if query
        .as_ref()
        .is_some_and(|q| q.len() > 32 * 1024 || q.contains(['\r', '\n']))
    {
        return crate::http_util::error(StatusCode::BAD_REQUEST, "Invalid query");
    }
    let bytes = match axum::body::to_bytes(body, BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::http_util::error(StatusCode::PAYLOAD_TOO_LARGE, "Body too large"),
    };
    let upstream_path = normalize(&path);
    let mut url = format!("{}{upstream_path}", state.image_host.trim_end_matches('/'));
    if let Some(query) = query.as_deref() {
        url.push('?');
        url.push_str(query);
    }
    let mut builder = state.http.request(parts.method.clone(), &url);
    if let Some(content_type) = parts.headers.get(header::CONTENT_TYPE) {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(accept) = parts.headers.get(header::ACCEPT) {
        builder = builder.header(header::ACCEPT, accept);
    }
    builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", state.nai_token));
    if !bytes.is_empty() {
        builder = builder.body(bytes);
    }
    let sent = match builder.send().await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(path = %upstream_path, error = %err, "upstream request failed");
            return crate::http_util::error(StatusCode::BAD_GATEWAY, "Upstream unavailable");
        }
    };
    let status = StatusCode::from_u16(sent.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    tracing::info!(method = %parts.method, path = %upstream_path, status = status.as_u16(), "proxied");
    let mut headers = HeaderMap::new();
    copy_header(&mut headers, sent.headers(), header::CONTENT_TYPE);
    copy_header(&mut headers, sent.headers(), header::RETRY_AFTER);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    let stream = sent.bytes_stream();
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

fn copy_header(dest: &mut HeaderMap, src: &HeaderMap, name: header::HeaderName) {
    if let Some(value) = src.get(&name) {
        dest.insert(name, value.clone());
    }
}

pub fn allow_subscription(raw: &Value, now: i64) -> Value {
    let Some(obj) = raw.as_object() else {
        return stub_subscription(now);
    };
    let steps = obj.get("trainingStepsLeft").cloned().unwrap_or(Value::Null);
    let steps = match steps {
        Value::Object(map) => {
            json!({
                "fixedTrainingStepsLeft": map.get("fixedTrainingStepsLeft").and_then(Value::as_i64).unwrap_or(0),
                "purchasedTrainingSteps": map.get("purchasedTrainingSteps").and_then(Value::as_i64).unwrap_or(0),
            })
        }
        _ => json!({"fixedTrainingStepsLeft": 0, "purchasedTrainingSteps": 0}),
    };
    let perks = obj.get("perks").cloned().unwrap_or(Value::Null);
    let perks = match perks {
        Value::Object(map) => json!({
            "imageGeneration": map.get("imageGeneration").and_then(Value::as_bool).unwrap_or(true),
            "unlimitedImageGeneration": map.get("unlimitedImageGeneration").and_then(Value::as_bool).unwrap_or(false),
            "voiceGeneration": map.get("voiceGeneration").and_then(Value::as_bool).unwrap_or(false),
            "contextTokens": map.get("contextTokens").and_then(Value::as_i64).unwrap_or(0),
        }),
        _ => json!({
            "imageGeneration": true,
            "unlimitedImageGeneration": false,
            "voiceGeneration": false,
            "contextTokens": 0
        }),
    };
    json!({
        "tier": obj.get("tier").and_then(Value::as_i64).unwrap_or(3),
        "active": obj.get("active").and_then(Value::as_bool).unwrap_or(true),
        "expiresAt": obj.get("expiresAt").and_then(Value::as_i64).unwrap_or(now + 3650 * 86400),
        "trainingStepsLeft": steps,
        "perks": perks,
    })
}

pub fn stub_subscription(now: i64) -> Value {
    json!({
        "tier": 3,
        "active": true,
        "expiresAt": now + 3650 * 86400,
        "trainingStepsLeft": {"fixedTrainingStepsLeft": 10000, "purchasedTrainingSteps": 0},
        "perks": {
            "imageGeneration": true,
            "unlimitedImageGeneration": true,
            "voiceGeneration": false,
            "contextTokens": 8192
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_covers_image_calls_only() {
        assert!(image_upstream(&Method::POST, "/ai/generate-image"));
        assert!(image_upstream(
            &Method::POST,
            "/v1/ai/generate-image-stream"
        ));
        assert!(image_upstream(&Method::POST, "/nai/ai/upscale"));
        assert!(image_upstream(
            &Method::GET,
            "/ai/generate-image/suggest-tags"
        ));
        assert!(!image_upstream(&Method::POST, "/ai/module/train"));
        assert!(!image_upstream(&Method::GET, "/ai/generate-image"));
        assert!(!image_upstream(&Method::POST, "/user/login"));
    }

    #[test]
    fn subscription_keeps_safe_fields_only() {
        let raw = json!({
            "tier": 1,
            "active": true,
            "expiresAt": 50,
            "email": "person@example.com",
            "username": "hidden-name",
            "paymentProcessorData": {"card": "4111"},
            "trainingStepsLeft": {"fixedTrainingStepsLeft": 12, "purchasedTrainingSteps": 1, "note": "secret"},
            "perks": {"imageGeneration": true, "unlimitedImageGeneration": false, "extra": "no"}
        });
        let clean = allow_subscription(&raw, 10);
        assert_eq!(clean["tier"], 1);
        assert_eq!(clean["trainingStepsLeft"]["fixedTrainingStepsLeft"], 12);
        assert!(clean.get("email").is_none());
        assert!(clean.get("username").is_none());
        assert!(clean.get("paymentProcessorData").is_none());
        assert!(clean["trainingStepsLeft"].get("note").is_none());
        assert!(clean["perks"].get("extra").is_none());
    }
}
