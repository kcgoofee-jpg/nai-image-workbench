use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::{Component, Path, PathBuf};

pub const BRIDGE_JS: &str = include_str!("bridge.js");
pub const I18N_JSON: &str = include_str!("../assets/workbench-i18n.json");

const IMAGE_CSP: &str =
    "default-src 'none'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; \
    worker-src 'self' blob:; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
    font-src 'self' https://fonts.gstatic.com data:; manifest-src 'self'; \
    connect-src 'self' data: blob: https://static.novelai.net; \
    img-src 'self' data: blob: https://static.novelai.net; \
    frame-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.contains('\0') {
        return None;
    }
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    let mut out = root.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return None,
        }
    }
    Some(out)
}

pub fn inject_bridge(html: &str) -> String {
    let tag = r#"<script src="/bridge.js"></script>"#;
    if html.contains("<!--owl-nai-boot-->") {
        return html.replacen("<!--owl-nai-boot-->", tag, 1);
    }
    if let Some(pos) = html.find("<head>") {
        let mut out = String::with_capacity(html.len() + tag.len());
        let at = pos + "<head>".len();
        out.push_str(&html[..at]);
        out.push_str(tag);
        out.push_str(&html[at..]);
        return out;
    }
    format!("{tag}{html}")
}

pub async fn image_page(mirror: &Path) -> Response {
    let Some(path) = safe_join(mirror, "image.html") else {
        return missing_mirror();
    };
    let html = match tokio::fs::read_to_string(&path).await {
        Ok(html) => html,
        Err(_) => return missing_mirror(),
    };
    let html = inject_bridge(&html);
    let mut response = Response::new(Body::from(html));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(IMAGE_CSP),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    response
}

pub async fn mirror_file(mirror: &Path, rel: &str) -> Response {
    if rel == "bridge.js" || rel == "workbench-i18n.json" || rel.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(path) = safe_join(mirror, rel) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(rel)),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}

pub fn bridge_js() -> Response {
    script_response(BRIDGE_JS, "application/javascript; charset=utf-8")
}

pub fn i18n_json() -> Response {
    script_response(I18N_JSON, "application/json; charset=utf-8")
}

fn script_response(body: &'static str, content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn missing_mirror() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "Place the image page files in MIRROR_DIR (image.html and its assets). This repository does not include them.",
    )
        .into_response()
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_rejects_escape() {
        let root = Path::new("/tmp/mirror");
        assert!(safe_join(root, "../.env").is_none());
        assert!(safe_join(root, "/_next/../../.env").is_none());
        assert!(safe_join(root, "image.html")
            .unwrap()
            .ends_with("image.html"));
        assert!(safe_join(root, "_next/static/a.js")
            .unwrap()
            .ends_with("_next/static/a.js"));
    }

    #[test]
    fn injects_bridge_without_secrets() {
        let html = inject_bridge("<head><title>image</title></head>");
        assert!(html.contains(r#"<script src="/bridge.js"></script>"#));
        assert!(!html.contains("NAI_TOKEN"));
        let marked = inject_bridge("<head><!--owl-nai-boot--></head>");
        assert_eq!(marked.matches("bridge.js").count(), 1);
    }
}
