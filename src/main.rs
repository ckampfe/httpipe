#![forbid(unsafe_code)]
// TODO
// - [ ] create named topics manually
// - [ ] terminology: id or topic?
// - [x] list topics
// - [ ] take away ability to disable autovivify, make it the only way
// - [x] option to autovivify channels
// - [x] option to autovivify pubsubs
// - [x] option on startup whether to allow autovivify or not
// - [x] update README for autovivify
// - [x] namespaces for channels, e.g., /channels/some_namespace/some_id
// - [ ] namespaces for pubsub, e.g., /channels/some_namespace/some_id
// - [ ] /c, /p shorthand endpoints for channels and pubsubs
// - [x] modules
// - [ ] reevalute API endpoints to be more RESTish
// - [ ] GET only API for browser stuff
// - [ ] add diagram to README to explain what httpipe is
// - [ ] rename to httq?

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

mod channel;
// mod pubsub;

/// when this is dropped it signals the oneshot channel
struct Done {
    tx: Option<oneshot::Sender<()>>,
}

impl Done {
    fn new() -> (Done, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        (Done { tx: Some(tx) }, rx)
    }
}

impl Drop for Done {
    fn drop(&mut self) {
        // needed because we can't move out of a &mut
        let tx = self
            .tx
            .take()
            .expect("this should never happen, it should always be Some");
        let _ = tx.send(());
    }
}

async fn app_state(State(state): State<Arc<AppState>>) -> axum::response::Result<String> {
    Ok(format!("{:#?}", state))
}

#[derive(Debug, Default)]
struct AppState {
    // pubsub_clients: pubsub::PubSubClients,
    channel_clients: channel::ChannelClients,
    options: Options,
}

#[derive(Parser, Debug)]
struct Options {
    /// the port to bind the server to
    #[arg(short, long, env, default_value = "3000")]
    port: u16,
    /// the maximum request timeout, in seconds
    #[arg(short, long, env)]
    request_timeout: Option<u64>,
    /// create named channels and pubsubs when they are first requested
    #[arg(short, long, env, default_value = "true")]
    autovivify: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            port: 3000,
            request_timeout: None,
            autovivify: true,
        }
    }
}

fn app(options: Options) -> axum::Router {
    let state = AppState {
        options,
        channel_clients: Mutex::new(HashMap::from([("default".to_string(), HashMap::new())])),
        ..Default::default()
    };
    let state = Arc::new(state);

    // let pubsub_routes = pubsub::routes();

    let channels_routes = channel::routes();

    let other_routes = Router::new().route("/state", get(app_state));

    let router = Router::new()
        // .merge(pubsub_routes)
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
