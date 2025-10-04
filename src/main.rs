#![forbid(unsafe_code)]
// TODO
// - [x] terminology: id or topic? channel_name
// - [x] list topics
// - [x] take away ability to disable autovivify, make it the only way
// - [x] option to autovivify channels
// - [x] option on startup whether to allow autovivify or not
// - [x] update README for autovivify
// - [x] namespaces for channels, e.g., /channels/some_namespace/some_id
// - [ ] /c, /p shorthand endpoints for channels
// - [x] modules
// - [x] automatically delete channels when unused
// - [x] automatically delete namespaces when unused
// - [ ] reevalute API endpoints to be more RESTish
// - [ ] GET only API for browser stuff
// - [ ] add diagram to README to explain what httpipe is
// - [ ] rename to httq?
// - [x] clean up topics/channels that have no use

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::Mutex;

mod channel;
mod drop_guard;

async fn app_state(
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let channel_clients = state.channel_clients.lock().await;
    let channel_stats: HashMap<_, _> = channel_clients
        .clone()
        .into_iter()
        .map(|(namespace, channels)| (namespace, channels.into_keys().collect::<Vec<_>>()))
        .collect();

    Ok(axum::Json(channel_stats))
}

#[derive(Debug, Default)]
struct AppState {
    channel_clients: channel::ChannelClients,
    options: Options,
}

#[derive(Parser, Debug)]
struct Options {
    /// the port to bind the server to
    #[arg(short, long, env, default_value = "6789")]
    port: u16,
    /// the maximum request timeout, in seconds
    #[arg(short, long, env)]
    request_timeout: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            port: 6789,
            request_timeout: None,
        }
    }
}

fn app(options: Options) -> axum::Router {
    let state = AppState {
        options,
        channel_clients: Mutex::new(HashMap::new()),
    };
    let state = Arc::new(state);

    let channels_routes = channel::routes(Arc::clone(&state));

    let other_routes = Router::new().route("/state", get(app_state));

    let router = Router::new()
        .merge(channels_routes)
        .merge(other_routes)
        .with_state(Arc::clone(&state))
        .layer(tower_http::normalize_path::NormalizePathLayer::trim_trailing_slash())
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::trace::TraceLayer::new_for_http());

    if let Some(request_timeout) = state.options.request_timeout {
        router.layer(tower_http::timeout::TimeoutLayer::new(
            std::time::Duration::from_secs(request_timeout),
        ))
    } else {
        router
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let options = Options::parse();

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", options.port)).await?;

    Ok(axum::serve(listener, app(options)).await?)
}

#[cfg(test)]
mod tests {}
