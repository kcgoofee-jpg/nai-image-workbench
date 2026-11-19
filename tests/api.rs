use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use nai_image_workbench::{router, test_state};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn image_page_has_no_token_and_missing_mirror_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(test_state(
        dir.path(),
        "http://127.0.0.1:9",
        "http://127.0.0.1:9",
    ));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/image")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("MIRROR_DIR"));
    assert!(!text.contains("upstream-token-for-tests"));
    assert!(!text.contains("0123456789abcdef"));
}

#[tokio::test]
async fn generation_requires_local_key_and_forwards_upstream_token_only() {
    let upstream = axum::Router::new()
        .route(
            "/ai/generate-image",
            axum::routing::post(|headers: axum::http::HeaderMap, body: String| async move {
                let auth = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                JsonOrText { auth, body }
            }),
        )
        .route(
            "/user/subscription",
            axum::routing::get(|| async {
                axum::Json(json!({
                    "tier": 2,
                    "active": true,
                    "expiresAt": 99,
                    "email": "person@example.com",
                    "trainingStepsLeft": {"fixedTrainingStepsLeft": 7, "purchasedTrainingSteps": 0},
                    "perks": {"imageGeneration": true, "unlimitedImageGeneration": false}
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let host = format!("http://{addr}");
    let dir = tempfile::tempdir().unwrap();
    let app = router(test_state(dir.path(), &host, &host));

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ai/generate-image")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"prompt":"secret prompt"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ai/generate-image")
                .header(header::AUTHORIZATION, "Bearer 0123456789abcdef")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"prompt":"secret prompt"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let forwarded = body_json(response).await;
    assert_eq!(forwarded["auth"], "Bearer upstream-token-for-tests");
    assert_eq!(forwarded["body"], r#"{"prompt":"secret prompt"}"#);
    assert!(!forwarded["auth"]
        .as_str()
        .unwrap()
        .contains("0123456789abcdef"));

    let blocked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ai/module/train")
                .header(header::AUTHORIZATION, "Bearer 0123456789abcdef")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::NOT_FOUND);

    let data = app
        .oneshot(
            Request::builder()
                .uri("/user/data")
                .header(header::AUTHORIZATION, "Bearer 0123456789abcdef")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(data.status(), StatusCode::OK);
    let data = body_json(data).await;
    assert_eq!(data["anlas"], 7);
    assert!(data.get("email").is_none());
    assert!(data["subscription"].get("email").is_none());
    let rendered = data.to_string();
    assert!(!rendered.contains("person@example.com"));
    assert!(!rendered.contains("upstream-token-for-tests"));
}

#[tokio::test]
async fn keystore_roundtrip_stays_local() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(test_state(
        dir.path(),
        "http://127.0.0.1:9",
        "http://127.0.0.1:9",
    ));
    let saved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/user/keystore")
                .header(header::AUTHORIZATION, "Bearer 0123456789abcdef")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"keystore":{"cipher":"local"},"changeIndex":3}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let loaded = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/user/keystore")
                .header(header::AUTHORIZATION, "Bearer 0123456789abcdef")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let loaded = body_json(loaded).await;
    assert_eq!(loaded["keystore"]["cipher"], "local");
    assert_eq!(loaded["changeIndex"], 3);

    let trial = app
        .oneshot(
            Request::builder()
                .uri("/ai/trial-status")
                .header(header::AUTHORIZATION, "Bearer 0123456789abcdef")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(trial.status(), StatusCode::OK);
    let trial = body_json(trial).await;
    assert_eq!(trial["remaining_image_actions"], 0);
}

struct JsonOrText {
    auth: String,
    body: String,
}

impl axum::response::IntoResponse for JsonOrText {
    fn into_response(self) -> axum::response::Response {
        axum::Json(json!({"auth": self.auth, "body": self.body})).into_response()
    }
}

#[test]
fn tree_has_no_private_markers() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut pending = vec![root.clone()];
    let banned = [
        "davidzhao",
        "discord",
        "pst-",
        "device-fp",
        "192.168.",
        "owl-gateway",
        "BEGIN OPENSSH",
        "BEGIN PRIVATE",
    ];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "target" || name == ".git" || name == "api.rs" {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if name.ends_with(".png") || name.ends_with(".woff2") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let lower = text.to_lowercase();
            for word in banned {
                assert!(
                    !lower.contains(word),
                    "{} contains {word}",
                    path.strip_prefix(&root).unwrap().display()
                );
            }
        }
    }
}
