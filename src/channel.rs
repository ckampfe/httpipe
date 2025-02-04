use crate::{AppState, Done};
use axum::body::{Body, BodyDataStream};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::Router;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

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

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/channels/namespaces", get(namespaces_index))
        .route("/channels/namespaces/{namespace}", post(namespace_create))
        .route("/channels/namespaces/{namespace}", get(namespace_index))
        .route("/channels/{namespace}/{id}", get(namespaced_subscribe))
        .route("/channels/{namespace}/{id}", post(namespaced_broadcast))
        .route("/channels", get(index))
        .route("/channels", post(create))
        .route("/channels/{id}", get(subscribe))
        .route("/channels/{id}", post(broadcast))
        .route("/channels/{id}", delete(close))
        .route(
            "/channels/{id}/subscribers_count",
            get(channel_subscriber_count),
        )
}

async fn index(
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<axum::Json<Vec<String>>> {
    let channel_clients = state.channel_clients.lock().await;

    let namespaced_channels = channel_clients.get("default").unwrap();

    Ok(axum::Json(
        namespaced_channels.keys().cloned().collect::<Vec<_>>(),
    ))
}

async fn namespaces_index(
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<axum::Json<Vec<String>>> {
    let channel_clients = state.channel_clients.lock().await;

    Ok(axum::Json(
        channel_clients.keys().cloned().collect::<Vec<_>>(),
    ))
}

async fn namespace_index(
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

async fn create(State(state): State<Arc<AppState>>) -> axum::response::Result<impl IntoResponse> {
    let mut channel_clients = state.channel_clients.lock().await;

    let namedspaced_channels = channel_clients.get_mut("default").unwrap();

    let channel_id = Uuid::new_v4().to_string();

    if let std::collections::hash_map::Entry::Vacant(e) =
        namedspaced_channels.entry(channel_id.to_string())
    {
        let (tx, rx) = flume::bounded(0);

        e.insert((tx, rx));

        Ok((StatusCode::CREATED, channel_id))
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR.into())
    }
}

async fn broadcast(
    request_headers: HeaderMap,
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    body: Body,
) -> axum::response::Result<()> {
    namespaced_broadcast(
        request_headers,
        Path(("default".to_string(), id)),
        State(state),
        body,
    )
    .await
}

async fn namespaced_broadcast(
    request_headers: HeaderMap,
    Path((namespace, id)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: Body,
) -> axum::response::Result<()> {
    let mut channel_clients = state.channel_clients.lock().await;

    let namespace_channels = if let Some(channels) = channel_clients.get_mut(&namespace) {
        channels
    } else if state.options.autovivify {
        channel_clients.insert(namespace.clone(), HashMap::new());
        channel_clients.get_mut(&namespace).unwrap()
    } else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    let tx = if let Some((tx, _rx)) = namespace_channels.get(&id) {
        tx.clone()
    } else if state.options.autovivify {
        let (tx, rx) = flume::bounded(0);

        namespace_channels.insert(id, (tx.clone(), rx));

        tx
    } else {
        return Err(StatusCode::NOT_FOUND.into());
    };

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
}

async fn subscribe(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    namespaced_subscribe(Path(("default".to_string(), id)), State(state)).await
}

async fn namespaced_subscribe(
    Path((namespace, id)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let mut channel_clients = state.channel_clients.lock().await;

    let namespace_channels = if let Some(channels) = channel_clients.get_mut(&namespace) {
        channels
    } else if state.options.autovivify {
        channel_clients.insert(namespace.clone(), HashMap::new());
        channel_clients.get_mut(&namespace).unwrap()
    } else {
        return Err(StatusCode::NOT_FOUND.into());
    };

    let rx = if let Some((_tx, rx)) = namespace_channels.get(&id) {
        rx.clone()
    } else if state.options.autovivify {
        let (tx, rx) = flume::bounded(0);

        namespace_channels.insert(id, (tx, rx.clone()));

        rx
    } else {
        return Err(StatusCode::NOT_FOUND.into());
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

async fn channel_subscriber_count(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<String> {
    let channel_clients = state.channel_clients.lock().await;

    let default_namespace = channel_clients.get("default").unwrap();

    if let Some((_tx, rx)) = default_namespace.get(&id) {
        Ok(rx.receiver_count().to_string())
    } else {
        Err(StatusCode::NOT_FOUND.into())
    }
}

async fn namespace_create(
    Path(namespace): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<impl IntoResponse> {
    let mut channel_clients = state.channel_clients.lock().await;

    if let std::collections::hash_map::Entry::Vacant(e) = channel_clients.entry(namespace.clone()) {
        e.insert(HashMap::new());

        Ok(StatusCode::CREATED)
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR.into())
    }
}

async fn close(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<()> {
    let mut channel_clients = state.channel_clients.lock().await;

    channel_clients.remove(&id);

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{app, Done, Options};
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

        let create_channel_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/channels"))
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

            reqwest::get(format!("http://localhost:{port}/channels/{id}"))
                .await
                .unwrap()
        });

        let post_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/channels/{id}"))
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
                .post(format!("http://localhost:{port}/channels"))
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

            reqwest::get(format!("http://localhost:{port}/channels/{id}"))
                .await
                .unwrap()
        });

        let post_handle = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("http://localhost:{port}/channels/{id}"))
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
                "http://localhost:{port}/channels/it_should_autovivify_on_receive"
            ))
            .await
            .unwrap()
        });

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/channels"))
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
                    "http://localhost:{port}/channels/it_should_autovivify_on_publish"
                ))
                .body("some body")
                .send()
                .await
                .unwrap()
        });

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/channels"))
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
                "http://localhost:{port}/channels/it_should_autovivify_on_receive"
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::NOT_FOUND
        );

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/channels"))
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
                    "http://localhost:{port}/channels/it_should_autovivify_on_publish"
                ))
                .body("some body")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );

        let ids: Vec<String> = reqwest::get(format!("http://localhost:{port}/channels"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(ids, Vec::<String>::new())
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

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let namespaces: HashSet<String> =
            reqwest::get(format!("http://localhost:{port}/channels/namespaces"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();

        assert_eq!(
            namespaces,
            HashSet::from(["default".to_string(), "a_great_ns".to_string()])
        );

        let ids: Vec<String> = reqwest::get(format!(
            "http://localhost:{port}/channels/namespaces/a_great_ns"
        ))
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

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let namespaces: HashSet<String> =
            reqwest::get(format!("http://localhost:{port}/channels/namespaces"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();

        assert_eq!(
            namespaces,
            HashSet::from(["default".to_string(), "a_great_ns".to_string()])
        );

        let ids: Vec<String> = reqwest::get(format!(
            "http://localhost:{port}/channels/namespaces/a_great_ns"
        ))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

        assert_eq!(ids, vec!["it_should_autovivify_on_publish"])
    }

    #[tokio::test]
    async fn namespaced_do_not_autovivify_on_receive_when_disabled() {
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
                "http://localhost:{port}/channels/a_great_ns/it_should_autovivify_on_receive"
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::NOT_FOUND
        );

        let namespaces: HashSet<String> =
            reqwest::get(format!("http://localhost:{port}/channels/namespaces"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();

        assert_eq!(namespaces, HashSet::from(["default".to_string()]));

        let ids = reqwest::get(format!(
            "http://localhost:{port}/channels/namespaces/a_great_ns"
        ))
        .await
        .unwrap();

        assert_eq!(ids.status(), StatusCode::NOT_FOUND)
    }
}
