mod auth;
mod config;
mod files;
mod http_util;
mod proxy;
mod state;
mod studio;

use crate::auth::AuthFail;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::Router;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;

pub use config::Config;
pub use state::AppState as SharedState;

const BODY_LIMIT: usize = 32 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/livez", get(health))
        .route("/image", get(image))
        .route("/workbench", get(workbench))
        .route("/bridge.js", get(|| async { files::bridge_js() }))
        .route("/workbench-i18n.json", get(|| async { files::i18n_json() }))
        .route("/ai/{*rest}", any(proxy::proxy_ai))
        .route("/v1/ai/{*rest}", any(proxy::proxy_ai))
        .route("/nai/ai/{*rest}", any(proxy::proxy_ai))
        .route("/ai/trial-status", get(studio::trial_status))
        .route("/user/data", get(studio::user_data))
        .route("/ai/user/data", get(studio::user_data))
        .route("/user/subscription", get(studio::user_subscription))
        .route("/ai/user/subscription", get(studio::user_subscription))
        .route("/user/priority", get(studio::user_priority))
        .route("/ai/user/priority", get(studio::user_priority))
        .route(
            "/user/keystore",
            get(studio::user_keystore).put(studio::user_keystore_put),
        )
        .route(
            "/ai/user/keystore",
            get(studio::user_keystore).put(studio::user_keystore_put),
        )
        .route(
            "/user/clientsettings",
            get(studio::user_settings).put(studio::user_settings_put),
        )
        .route(
            "/ai/user/clientsettings",
            get(studio::user_settings).put(studio::user_settings_put),
        )
        .route("/user/inventory", get(studio::user_inventory))
        .route("/ai/user/inventory", get(studio::user_inventory))
        .route("/user/login", post(studio::user_login))
        .route("/ai/user/login", post(studio::user_login))
        .route("/user/information", get(studio::user_information))
        .route("/ai/user/information", get(studio::user_information))
        .route("/user/giftkeys", get(studio::empty_list))
        .route("/user/self", get(studio::empty_object))
        .route("/user/cb/get-customer", get(studio::empty_object))
        .route(
            "/usage/link",
            get(studio::empty_object).post(studio::empty_object),
        )
        .route(
            "/usage/event",
            get(studio::empty_object).post(studio::empty_object),
        )
        .route("/user/objects", get(studio::empty_objects))
        .route("/user/objects/{kind}", get(studio::empty_objects))
        .route("/public/tokenizer/{model}", get(studio::public_tokenizer))
        .fallback(static_fallback)
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::X_CONTENT_TYPE_OPTIONS,
            axum::http::HeaderValue::from_static("nosniff"),
        ))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_access,
        ))
        .with_state(state)
}

async fn health() -> Response {
    http_util::ok(serde_json::json!({"ok": true}))
}

async fn image(State(state): State<AppState>) -> Response {
    files::image_page(&state.mirror).await
}

async fn workbench() -> Response {
    axum::response::Redirect::permanent("/image").into_response()
}

async fn static_fallback(State(state): State<AppState>, req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    if static_allowed(path) {
        files::mirror_file(&state.mirror, path).await
    } else {
        http_util::error(axum::http::StatusCode::NOT_FOUND, "Not Found")
    }
}

fn static_allowed(path: &str) -> bool {
    path.starts_with("_next/")
        || path.starts_with("js/")
        || path.starts_with("icons/")
        || path.starts_with("cf-fonts/")
        || path.starts_with("tokenizer/")
        || matches!(
            path,
            "manifest.json" | "updateReload.json" | "favicon.ico" | "favicon.svg"
        )
}

fn access_exempt(method: &axum::http::Method, path: &str) -> bool {
    if matches!(path, "/user/login" | "/ai/user/login") && *method == axum::http::Method::POST {
        return true;
    }
    if !matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD) {
        return false;
    }
    matches!(
        path,
        "/healthz" | "/livez" | "/image" | "/workbench" | "/bridge.js" | "/workbench-i18n.json"
    ) || static_allowed(path.trim_start_matches('/'))
}

async fn require_access(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if access_exempt(req.method(), req.uri().path()) {
        return next.run(req).await;
    }
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    match state.authorize(header) {
        Ok(()) => next.run(req).await,
        Err(AuthFail::Limited) => http_util::error(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts",
        ),
        Err(AuthFail::Denied) => {
            http_util::error(axum::http::StatusCode::UNAUTHORIZED, "Unauthorized")
        }
    }
}

pub fn test_state(dir: &std::path::Path, image_host: &str, api_host: &str) -> AppState {
    AppState::new(
        "0123456789abcdef".into(),
        "upstream-token-for-tests".into(),
        image_host.into(),
        api_host.into(),
        dir.join("mirror"),
        &dir.join("studio.db"),
    )
    .expect("test state")
}
