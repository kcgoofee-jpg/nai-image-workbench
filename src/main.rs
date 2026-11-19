use nai_image_workbench::Config;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
    let bind = config.bind.clone();
    let db_path = config.data_dir.join("studio.db");
    let state = match nai_image_workbench::SharedState::new(
        config.access_key,
        config.nai_token,
        config.image_host,
        config.api_host,
        config.mirror,
        &db_path,
    ) {
        Ok(state) => state,
        Err(err) => {
            eprintln!("startup failed: {err}");
            std::process::exit(1);
        }
    };
    let app = nai_image_workbench::router(state);
    let listener = match TcpListener::bind(&bind).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("bind {bind} failed: {err}");
            std::process::exit(1);
        }
    };
    eprintln!("listening on http://{bind}");
    if let Err(err) = axum::serve(listener, app).await {
        eprintln!("server stopped: {err}");
        std::process::exit(1);
    }
}
