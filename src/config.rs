use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub access_key: String,
    pub nai_token: String,
    pub mirror: PathBuf,
    pub data_dir: PathBuf,
    pub bind: String,
    pub image_host: String,
    pub api_host: String,
    pub allow_lan: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        load_dotenv();
        let access_key = required("ACCESS_KEY")?;
        let nai_token = required("NAI_TOKEN")?;
        if access_key.chars().count() < 16 {
            return Err("ACCESS_KEY must be at least 16 characters".into());
        }
        if access_key == nai_token {
            return Err("ACCESS_KEY must not be the same as NAI_TOKEN".into());
        }
        let image_host = env_or("NAI_IMAGE_HOST", "https://image.novelai.net");
        let api_host = env_or("NAI_API_HOST", "https://api.novelai.net");
        check_base(&image_host, "NAI_IMAGE_HOST")?;
        check_base(&api_host, "NAI_API_HOST")?;
        let bind = env_or("BIND", "127.0.0.1:8787");
        let allow_lan = std::env::var("ALLOW_LAN").ok().as_deref() == Some("1");
        if !allow_lan && !loopback_bind(&bind) {
            return Err(
                "BIND is not a loopback address. Set ALLOW_LAN=1 only if you intend to expose it."
                    .into(),
            );
        }
        Ok(Self {
            access_key,
            nai_token,
            mirror: PathBuf::from(env_or("MIRROR_DIR", "./mirror/nai-app")),
            data_dir: PathBuf::from(env_or("DATA_DIR", "./data")),
            bind,
            image_host,
            api_host,
            allow_lan,
        })
    }
}

pub fn check_base(raw: &str, name: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|_| format!("{name} is not a URL"))?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(format!("{name} must be http or https"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{name} must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() || (url.path() != "/" && url.path() != "")
    {
        return Err(format!("{name} must be an origin without a path"));
    }
    let host = url.host_str().unwrap_or("");
    if host.is_empty() || host == "169.254.169.254" || host == "metadata.google.internal" {
        return Err(format!("{name} host is not allowed"));
    }
    if url.scheme() == "http" && !is_loopback_host(host) {
        return Err(format!("{name} may use http only for localhost"));
    }
    Ok(())
}

pub fn loopback_bind(bind: &str) -> bool {
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    let host = host.trim_matches(['[', ']']);
    is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

fn required(name: &str) -> Result<String, String> {
    let value = std::env::var(name).unwrap_or_default();
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(format!("{name} is required"));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(format!("{name} contains a control character"));
    }
    Ok(value)
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn load_dotenv() {
    let path = Path::new(".env");
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue;
        }
        let mut value = value.trim().to_string();
        if value.len() >= 2 {
            let bytes = value.as_bytes();
            if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
                || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            {
                value = value[1..value.len() - 1].to_string();
            }
        }
        // SAFETY: set_var is process-global. This runs once before threads start.
        unsafe { std::env::set_var(key, value) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_novelai_and_local_origins() {
        check_base("https://image.novelai.net", "NAI_IMAGE_HOST").unwrap();
        check_base("http://127.0.0.1:9", "NAI_IMAGE_HOST").unwrap();
    }

    #[test]
    fn rejects_credential_path_and_metadata_hosts() {
        assert!(check_base("https://user:pass@image.novelai.net", "H").is_err());
        assert!(check_base("https://image.novelai.net/ai", "H").is_err());
        assert!(check_base("http://example.com", "H").is_err());
        assert!(check_base("https://169.254.169.254", "H").is_err());
        assert!(check_base("file:///tmp/x", "H").is_err());
    }

    #[test]
    fn loopback_detection() {
        assert!(loopback_bind("127.0.0.1:8787"));
        assert!(loopback_bind("[::1]:8787"));
        assert!(!loopback_bind("0.0.0.0:8787"));
    }
}
