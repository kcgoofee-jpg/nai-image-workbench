use crate::auth::{self, AuthFail};
use crate::studio::StudioDb;
use reqwest::redirect::Policy;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct AppState {
    pub access_key: String,
    pub nai_token: String,
    pub image_host: String,
    pub api_host: String,
    pub mirror: PathBuf,
    pub http: reqwest::Client,
    pub db: Arc<StudioDb>,
    pub subscription: Arc<Mutex<Option<(Instant, Value)>>>,
    pub failures: Arc<Mutex<Vec<Instant>>>,
}

impl AppState {
    pub fn new(
        access_key: String,
        nai_token: String,
        image_host: String,
        api_host: String,
        mirror: PathBuf,
        db_path: &std::path::Path,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            access_key,
            nai_token,
            image_host,
            api_host,
            mirror,
            http,
            db: Arc::new(StudioDb::open(db_path)?),
            subscription: Arc::new(Mutex::new(None)),
            failures: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn authorize(&self, header: Option<&str>) -> Result<(), AuthFail> {
        if auth::keys_match(auth::bearer(header), &self.access_key) {
            return Ok(());
        }
        if self.recent_failures() >= 30 {
            return Err(AuthFail::Limited);
        }
        self.record_failure();
        Err(AuthFail::Denied)
    }

    pub fn record_failure(&self) {
        let mut guard = self.failures.lock().expect("failure counter");
        let now = Instant::now();
        guard.retain(|at| now.duration_since(*at) < Duration::from_secs(300));
        guard.push(now);
    }

    fn recent_failures(&self) -> usize {
        let mut guard = self.failures.lock().expect("failure counter");
        let now = Instant::now();
        guard.retain(|at| now.duration_since(*at) < Duration::from_secs(300));
        guard.len()
    }
}
