#![forbid(unsafe_code)]

use axum::body::{Body, BodyDataStream, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::Router;
use clap::Parser;
use rand::distr::{Alphanumeric, SampleString};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio_stream::StreamExt;

async fn pubsub_create(State(state): State<Arc<AppState>>) -> axum::response::Result<String> {
    let mut pubsub_clients = state.pubsub_clients.lock().await;

    let channel_id = Alphanumeric.sample_string(&mut rand::rng(), 20);

    if !pubsub_clients.contains_key(&channel_id) {
        let (tx, _rx) = broadcast::channel(5);

        pubsub_clients.insert(channel_id.clone(), tx);

        Ok(channel_id)
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR.into())
    }
}

async fn pubsub_broadcast(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    body: Body,
) -> axum::response::Result<()> {
    let pubsub_clients = state.pubsub_clients.lock().await;

    let mut body_stream = body.into_data_stream();

    if let Some(tx) = pubsub_clients.get(&id) {
        while let Some(frame) = body_stream.next().await {
            let bytes = frame.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let _ = tx.send(Message::Data(bytes));
        }

        let _ = tx.send(Message::Close);

        Ok(())
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

async fn pubsub_subscribe(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let pubsub_clients = state.pubsub_clients.lock().await;

    if let Some(tx) = pubsub_clients.get(&id) {
        let rx = tx.subscribe();

        let stream = tokio_stream::wrappers::BroadcastStream::new(rx);

        let stream = stream
            .map(|m| {
                let m = m.unwrap();
                match m {
                    Message::Data(s) => Some(Ok::<Bytes, String>(s)),
                    Message::Close => None,
                }
            })
            .take_while(|item| item.is_some())
            .map(|item| item.unwrap());

        let body = Body::from_stream(stream);

        let headers = [(header::CONTENT_TYPE, "text/plain")];

        Ok((headers, body))
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

async fn pubsub_close(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<()> {
    let mut pubsub_clients = state.pubsub_clients.lock().await;

    if let Some(tx) = pubsub_clients.get(&id) {
        let _ = tx.send(Message::Close);
    }

    pubsub_clients.remove(&id);

    Ok(())
}

async fn pubsub_count(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<String> {
    let pubsub_clients = state.pubsub_clients.lock().await;

    if let Some(tx) = pubsub_clients.get(&id) {
        let subscribers_count = tx.receiver_count();
        Ok(subscribers_count.to_string())
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

async fn channel_create(
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let mut channel_clients = state.channel_clients.lock().await;

    let channel_id = Alphanumeric.sample_string(&mut rand::rng(), 20);

    if !channel_clients.contains_key(&channel_id) {
        let (tx, rx) = flume::bounded(0);

        channel_clients.insert(channel_id.clone(), (tx, rx));

        Ok((StatusCode::CREATED, channel_id))
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR.into())
    }
}

async fn channel_broadcast(
    request_headers: HeaderMap,
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    body: Body,
) -> axum::response::Result<()> {
    let channel_clients = state.channel_clients.lock().await;

    if let Some((tx, _rx)) = channel_clients.get(&id) {
        let tx = tx.clone();

        drop(channel_clients);

        let body_stream = body.into_data_stream();

        let (done, done_rx) = Done::new();

        tx.send_async((body_stream, request_headers, done))
            .await
            .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

        done_rx
            .await
            .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok(())
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

/// when this is dropped, it signals to the producer that we're done,
/// and it can return 200 OK
struct Done {
    tx: Option<oneshot::Sender<()>>,
}

impl Done {
    fn new() -> (Done, tokio::sync::oneshot::Receiver<()>) {
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

async fn channel_subscribe(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let channel_clients = state.channel_clients.lock().await;

    if let Some((_tx, rx)) = channel_clients.get(&id) {
        let rx = rx.clone();

        drop(channel_clients);

        let rx = rx.into_recv_async();

        let (stream, request_headers, _done) =
            rx.await.map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

        let body = Body::from_stream(stream);

        if let Some(content_type) = request_headers.get(header::CONTENT_TYPE) {
            let headers = [(header::CONTENT_TYPE, content_type)];
            Ok((headers, body).into_response())
        } else {
            Ok(body.into_response())
        }
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

async fn channel_subscriber_count(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<String> {
    let channel_clients = state.channel_clients.lock().await;
    if let Some((_tx, rx)) = channel_clients.get(&id) {
        Ok(rx.receiver_count().to_string())
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

async fn channel_close(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<()> {
    let mut channel_clients = state.channel_clients.lock().await;

    channel_clients.remove(&id);

    Ok(())
}

#[derive(Clone)]
enum Message {
    Data(Bytes),
    Close,
}

type PubSubClients = Mutex<HashMap<String, broadcast::Sender<Message>>>;

type ChannelClients = Mutex<
    HashMap<
        String,
        (
            flume::Sender<(BodyDataStream, HeaderMap, Done)>,
            flume::Receiver<(BodyDataStream, HeaderMap, Done)>,
        ),
    >,
>;

#[derive(Debug, Default)]
struct AppState {
    pubsub_clients: PubSubClients,
    channel_clients: ChannelClients,
}

#[derive(Parser)]
struct Options {
    #[arg(short, long, env, default_value = "3000")]
    port: u16,
}

fn make_app() -> axum::Router {
    let state = Arc::new(AppState::default());

    let pubsub_routes = Router::new()
        .route("/pubsubs", post(pubsub_create))
        .route("/pubsubs/{id}", get(pubsub_subscribe))
        .route("/pubsubs/{id}", post(pubsub_broadcast))
        .route("/pubsubs/{id}", delete(pubsub_close))
        .route("/pubsubs/{id}/subscribers_count", get(pubsub_count));

    let channels_routes = Router::new()
        .route("/channels", post(channel_create))
        .route("/channels/{id}", get(channel_subscribe))
        .route("/channels/{id}", post(channel_broadcast))
        .route("/channels/{id}", delete(channel_close))
        .route(
            "/channels/{id}/subscribers_count",
            get(channel_subscriber_count),
        );

    Router::new()
        .merge(pubsub_routes)
        .merge(channels_routes)
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse();

    let app = make_app();

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", options.port)).await?;

    Ok(axum::serve(listener, app).await?)
}

#[cfg(test)]
mod tests {
    use crate::{make_app, Done};
    use reqwest::StatusCode;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestMessage {
        greeting: String,
    }

    #[tokio::test]
    async fn simple_channels_work() {
        let test_message = TestMessage {
            greeting: "Bwah!".to_string(),
        };

        let test_message_clone = test_message.clone();

        let app = make_app();

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", 3000))
            .await
            .unwrap();

        let (_done, done_rx) = Done::new();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { done_rx.await.unwrap() })
                .await
                .unwrap();
        });

        let create_channel_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post("http://localhost:3000/channels")
                .send()
                .await
                .unwrap()
        });

        let create_channel_response = create_channel_handle.await.unwrap();

        assert_eq!(create_channel_response.status(), StatusCode::CREATED);
        let id = create_channel_response.text().await.unwrap();
        assert_eq!(id.len(), 20);

        let id_clone = id.clone();

        let get_handle = tokio::spawn(async move {
            let id = id_clone;

            reqwest::get(format!("http://localhost:3000/channels/{id}"))
                .await
                .unwrap()
        });

        let post_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:3000/channels/{id}"))
                .json(&test_message)
                .send()
                .await
                .unwrap()
        });

        let get_response = get_handle.await.unwrap();
        let post_response = post_handle.await.unwrap();

        assert_eq!(post_response.status(), StatusCode::OK);
        assert_eq!(get_response.status(), StatusCode::OK);

        assert_eq!(
            get_response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .unwrap(),
            "application/json"
        );
        let get_response_text = get_response.text().await.unwrap();
        let get_response_parsed: TestMessage = serde_json::from_str(&get_response_text).unwrap();
        assert_eq!(get_response_parsed, test_message_clone);
    }
}
