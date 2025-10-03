use crate::{AppState, Done};
use axum::Router;
use axum::body::{Body, BodyDataStream};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

type Namespace = String;
type ChannelName = String;
pub(crate) type ChannelClients = Mutex<
    HashMap<
        Namespace,
        HashMap<
            ChannelName,
            (
                flume::Sender<(BodyDataStream, HeaderMap, Done)>,
                flume::Receiver<(BodyDataStream, HeaderMap, Done)>,
            ),
        >,
    >,
>;

pub(crate) fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/channels/namespaces", get(list_all_namespaces))
        .route("/channels/{namespace}", get(list_all_namespace_channels))
        .route(
            "/channels/{namespace}/{channel_name}",
            get(subscribe_to_channel).route_layer(middleware::from_fn_with_state(
                state.clone(),
                clean_up_unused_channels,
            )),
        )
        .route(
            "/channels/{namespace}/{channel_name}",
            post(broadcast_to_channel).route_layer(middleware::from_fn_with_state(
                state.clone(),
                clean_up_unused_channels,
            )),
        )
}

async fn clean_up_unused_channels(
    Path((namespace, channel_name)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let _ = rx.await;

        let mut channel_clients = state.channel_clients.lock().await;

        if let Some(namespace_channels) = channel_clients.get_mut(&namespace) {
            if let Some((tx, _rx)) = namespace_channels.get(&channel_name) {
                if tx.sender_count() <= 1 && tx.receiver_count() <= 1 {
                    namespace_channels.remove(&channel_name);
                }
            }

            if namespace_channels.is_empty() {
                channel_clients.remove(&namespace);
            }
        }
    });

    let response = next.run(request).await;

    let _ = tx.send(());

    response
}

async fn list_all_namespaces(
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<axum::Json<Vec<String>>> {
    let channel_clients = state.channel_clients.lock().await;

    Ok(axum::Json(
        channel_clients.keys().cloned().collect::<Vec<_>>(),
    ))
}

async fn list_all_namespace_channels(
    Path(namespace): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<axum::Json<Vec<String>>> {
    let channel_clients = state.channel_clients.lock().await;

    let namespaced_channels = if let Some(channels) = channel_clients.get(&namespace) {
        channels
    } else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    Ok(axum::Json(
        namespaced_channels.keys().cloned().collect::<Vec<_>>(),
    ))
}

async fn broadcast_to_channel(
    request_headers: HeaderMap,
    Path((namespace, channel_name)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: Body,
) -> axum::response::Result<()> {
    let mut channel_clients = state.channel_clients.lock().await;

    let namespace_channels = match channel_clients.entry(namespace) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => e.insert(HashMap::new()),
    };

    let tx = if let Some((tx, _rx)) = namespace_channels.get(&channel_name) {
        tx.clone()
    } else {
        let (tx, rx) = flume::bounded(0);

        namespace_channels.insert(channel_name, (tx.clone(), rx));

        tx
    };

    drop(channel_clients);

    let request_body_stream = body.into_data_stream();

    let (done, done_rx) = Done::new();

    tx.send_async((request_body_stream, request_headers, done))
        .await
        .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

    done_rx
        .await
        .map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(())
}

async fn subscribe_to_channel(
    Path((namespace, channel_name)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let mut channel_clients = state.channel_clients.lock().await;

    let namespace_channels = match channel_clients.entry(namespace) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => e.insert(HashMap::new()),
    };

    let rx = if let Some((_tx, rx)) = namespace_channels.get(&channel_name) {
        rx.clone()
    } else {
        let (tx, rx) = flume::bounded(0);

        namespace_channels.insert(channel_name, (tx, rx.clone()));

        rx
    };

    drop(channel_clients);

    let rx = rx.into_recv_async();

    let (stream, producer_request_headers, _done) =
        rx.await.map_err(|_e| StatusCode::INTERNAL_SERVER_ERROR)?;

    let body = Body::from_stream(stream);

    // we do this because by default, POSTs from curl are `x-www-form-urlencoded`
    let producer_content_type = producer_request_headers
        .get(header::CONTENT_TYPE)
        .cloned()
        .and_then(|header_value| {
            // TODO should we also reject multipart/form-data?
            // TODO should we use mime crate?
            if header_value == "application/x-www-form-urlencoded" {
                None
            } else {
                Some(header_value)
            }
        })
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));

    let headers = [(header::CONTENT_TYPE, producer_content_type)];

    Ok((headers, body).into_response())
}

#[cfg(test)]
mod tests {
    use crate::{Done, Options, app};
    use axum::http::StatusCode;
    use serde::{Deserialize, Serialize};
    use std::{collections::HashSet, sync::atomic::AtomicU16};

    static PORT: AtomicU16 = AtomicU16::new(4000);

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

        let get_handle = tokio::spawn(async move {
            reqwest::get(format!("http://localhost:{port}/channels/v1/hello"))
                .await
                .unwrap()
        });

        let post_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/channels/v1/hello"))
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

        let namespaces: Vec<String> =
            reqwest::get(format!("http://localhost:{port}/channels/namespaces"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();

        assert_eq!(namespaces, Vec::<String>::new());

        let ids = reqwest::get(format!("http://localhost:{port}/channels/a_great_ns"))
            .await
            .unwrap();

        assert_eq!(ids.status(), StatusCode::NOT_FOUND)
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

        let get_handle = tokio::spawn(async move {
            reqwest::get(format!("http://localhost:{port}/channels/v1/hello"))
                .await
                .unwrap()
        });

        let post_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/channels/v1/hello"))
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
    async fn namespaced_autovivify_on_receive() {
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
                "http://localhost:{port}/channels/a_great_ns/it_should_autovivify_on_receive"
            ))
            .await
            .unwrap()
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let namespaces: HashSet<String> =
            reqwest::get(format!("http://localhost:{port}/channels/namespaces"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();

        assert_eq!(namespaces, HashSet::from(["a_great_ns".to_string()]));

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/channels/a_great_ns"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(ids, vec!["it_should_autovivify_on_receive"])
    }

    #[tokio::test]
    async fn namespaced_autovivify_on_publish() {
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
                    "http://localhost:{port}/channels/a_great_ns/it_should_autovivify_on_publish"
                ))
                .body("some body")
                .send()
                .await
                .unwrap()
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let namespaces: HashSet<String> =
            reqwest::get(format!("http://localhost:{port}/channels/namespaces"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();

        assert_eq!(namespaces, HashSet::from(["a_great_ns".to_string()]));

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/channels/a_great_ns"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(ids, vec!["it_should_autovivify_on_publish"])
    }

    // #[tokio::test]
    // async fn auto_cleanup_does_not_delete_when_multiple_connections_exist()
}
