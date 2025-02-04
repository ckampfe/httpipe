use crate::AppState;
use axum::body::{Body, Bytes};
use axum::extract::Path;
use axum::http::{header, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::Router;
use axum::{extract::State, response::IntoResponse};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio_stream::StreamExt;
use uuid::Uuid;

pub(crate) type PubSubClients = Mutex<HashMap<String, broadcast::Sender<Message>>>;

#[derive(Clone)]
pub(crate) enum Message {
    Headers(HeaderMap),
    Data(Bytes),
    Close,
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/pubsub", get(pubsub_index))
        .route("/pubsub", post(pubsub_create))
        .route("/pubsub/{id}", get(pubsub_subscribe))
        .route("/pubsub/{id}", post(pubsub_broadcast))
        .route("/pubsub/{id}", delete(pubsub_close))
        .route("/pubsub/{id}/subscribers_count", get(pubsub_count))
}

async fn pubsub_index(
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<axum::Json<Vec<String>>> {
    let pubsub_clients = state.pubsub_clients.lock().await;

    Ok(axum::Json(
        pubsub_clients.keys().cloned().collect::<Vec<_>>(),
    ))
}

async fn pubsub_create(
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let mut pubsub_clients = state.pubsub_clients.lock().await;

    let channel_id = Uuid::new_v4().to_string();

    if let std::collections::hash_map::Entry::Vacant(e) = pubsub_clients.entry(channel_id.clone()) {
        let (tx, _rx) = broadcast::channel(5);

        e.insert(tx);

        Ok((StatusCode::CREATED, channel_id))
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR.into())
    }
}

async fn pubsub_broadcast(
    request_headers: HeaderMap,
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    body: Body,
) -> axum::response::Result<()> {
    let mut pubsub_clients = state.pubsub_clients.lock().await;

    let mut body_stream = body.into_data_stream();

    let tx = if let Some(tx) = pubsub_clients.get(&id) {
        tx.clone()
    } else if state.options.autovivify {
        let (tx, _rx) = broadcast::channel(5);
        pubsub_clients.insert(id, tx.clone());
        tx
    } else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    drop(pubsub_clients);

    tx.send(Message::Headers(request_headers))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    while let Some(frame) = body_stream.next().await {
        let bytes = frame.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let _ = tx.send(Message::Data(bytes));
    }

    let _ = tx.send(Message::Close);

    Ok(())
}

async fn pubsub_subscribe(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let mut pubsub_clients = state.pubsub_clients.lock().await;

    let tx = if let Some(tx) = pubsub_clients.get(&id) {
        tx.clone()
    } else if state.options.autovivify {
        let (tx, _rx) = broadcast::channel(5);
        pubsub_clients.insert(id, tx.clone());
        tx
    } else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    drop(pubsub_clients);

    let rx = tx.subscribe();

    let mut stream = tokio_stream::wrappers::BroadcastStream::new(rx);

    let producer_request_headers = if let Some(Ok(Message::Headers(headers))) = stream.next().await
    {
        headers
    } else {
        return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
    };

    // TODO figure out how to get producer headers here
    // so we can appropriately forward on the headers
    let stream = stream
        .map(|m| {
            let m = m.unwrap();
            match m {
                Message::Data(s) => Some(Ok::<Bytes, String>(s)),
                Message::Close => None,
                Message::Headers(_) => panic!(
                    "we should never receive headers here, always before the main data stream"
                ),
            }
        })
        .take_while(|item| item.is_some())
        .map(|item| item.unwrap());

    let body = Body::from_stream(stream);

    let producer_content_type = producer_request_headers
        .get(header::CONTENT_TYPE)
        .cloned()
        .and_then(|header_value| {
            // TODO should we also reject multipart/form-data?
            if header_value == "application/x-www-form-urlencoded" {
                None
            } else {
                Some(header_value)
            }
        })
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));

    let headers = [(header::CONTENT_TYPE, producer_content_type)];

    Ok((headers, body))
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

#[cfg(test)]
mod tests {
    use crate::{app, Done, Options};
    use axum::http::StatusCode;
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::AtomicU16;

    static PORT: AtomicU16 = AtomicU16::new(3000);

    fn get_port() -> u16 {
        PORT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestMessage {
        greeting: String,
    }

    #[tokio::test]
    async fn simple_json() {
        let test_message = TestMessage {
            greeting: "Bwah!".to_string(),
        };

        let test_message_clone = test_message.clone();

        let options = Options::default();

        let port = get_port();

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .unwrap();

        let (_done, done_rx) = Done::new();

        tokio::spawn(async move {
            axum::serve(listener, app(options))
                .with_graceful_shutdown(async move { done_rx.await.unwrap() })
                .await
                .unwrap();
        });

        let create_channel_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/pubsub"))
                .send()
                .await
                .unwrap()
        });

        let create_channel_response = create_channel_handle.await.unwrap();

        assert_eq!(create_channel_response.status(), StatusCode::CREATED);
        let id = create_channel_response.text().await.unwrap();
        assert_eq!(id.len(), 36);

        let id_clone = id.clone();

        let get_handle = tokio::spawn(async move {
            let id = id_clone;

            reqwest::get(format!("http://localhost:{port}/pubsub/{id}"))
                .await
                .unwrap()
        });

        // post is nonblocking, get is blocking,
        // so we need to ensure that get is set up and blocking
        // before we run the post
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let post_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/pubsub/{id}"))
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

    #[tokio::test]
    async fn content_type_is_application_octet_stream_unless_otherwise_specified() {
        let test_message = "hello";

        let options = Options::default();

        let port = get_port();

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .unwrap();

        let (_done, done_rx) = Done::new();

        tokio::spawn(async move {
            axum::serve(listener, app(options))
                .with_graceful_shutdown(async move { done_rx.await.unwrap() })
                .await
                .unwrap();
        });

        let create_channel_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/pubsub"))
                .send()
                .await
                .unwrap()
        });

        let create_channel_response = create_channel_handle.await.unwrap();

        assert_eq!(create_channel_response.status(), StatusCode::CREATED);
        let id = create_channel_response.text().await.unwrap();
        assert_eq!(id.len(), 36);

        let id_clone = id.clone();

        let get_handle = tokio::spawn(async move {
            let id = id_clone;

            reqwest::get(format!("http://localhost:{port}/pubsub/{id}"))
                .await
                .unwrap()
        });

        // post is nonblocking, get is blocking,
        // so we need to ensure that get is set up and blocking
        // before we run the post
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let post_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/pubsub/{id}"))
                .body(test_message)
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
            "application/octet-stream"
        );
        let get_response_text = get_response.text().await.unwrap();
        assert_eq!(get_response_text, test_message);
    }

    #[tokio::test]
    async fn autovivify_on_receive() {
        let options = Options::default();

        let port = get_port();

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .unwrap();

        let (_done, done_rx) = Done::new();

        tokio::spawn(async move {
            axum::serve(listener, app(options))
                .with_graceful_shutdown(async move { done_rx.await.unwrap() })
                .await
                .unwrap();
        });

        tokio::spawn(async move {
            reqwest::get(format!(
                "http://localhost:{port}/pubsub/it_should_autovivify_on_receive"
            ))
            .await
            .unwrap()
        });

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/pubsub"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(ids, vec!["it_should_autovivify_on_receive"])
    }

    #[tokio::test]
    async fn autovivify_on_publish() {
        let options = Options::default();

        let port = get_port();

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .unwrap();

        let (_done, done_rx) = Done::new();

        tokio::spawn(async move {
            axum::serve(listener, app(options))
                .with_graceful_shutdown(async move { done_rx.await.unwrap() })
                .await
                .unwrap();
        });

        tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!(
                    "http://localhost:{port}/pubsub/it_should_autovivify_on_publish"
                ))
                .body("some body")
                .send()
                .await
                .unwrap()
        });

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/pubsub"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(ids, vec!["it_should_autovivify_on_publish"])
    }

    #[tokio::test]
    async fn do_not_autovivify_on_receive_when_disabled() {
        let options = Options {
            autovivify: false,
            ..Default::default()
        };

        let port = get_port();

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .unwrap();

        let (_done, done_rx) = Done::new();

        tokio::spawn(async move {
            axum::serve(listener, app(options))
                .with_graceful_shutdown(async move { done_rx.await.unwrap() })
                .await
                .unwrap();
        });

        assert_eq!(
            reqwest::get(format!(
                "http://localhost:{port}/pubsub/it_should_autovivify_on_receive"
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::NOT_FOUND
        );

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/pubsub"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert!(ids.is_empty())
    }

    #[tokio::test]
    async fn do_not_autovivify_on_publish_when_disabled() {
        let options = Options {
            autovivify: false,
            ..Default::default()
        };

        let port = get_port();

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .unwrap();

        let (_done, done_rx) = Done::new();

        tokio::spawn(async move {
            axum::serve(listener, app(options))
                .with_graceful_shutdown(async move { done_rx.await.unwrap() })
                .await
                .unwrap();
        });

        assert_eq!(
            reqwest::Client::new()
                .post(format!(
                    "http://localhost:{port}/pubsub/it_should_autovivify_on_publish"
                ))
                .body("some body")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/pubsub"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert!(ids.is_empty())
    }
}
