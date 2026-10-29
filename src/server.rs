//! The localhost HTTP API. Every route needs the bearer token; a request from a browser (an `Origin` header)
//! or with a foreign `Host` is refused, which closes the DNS-rebinding and cross-site routes to a local server.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::{Cmd, NewNotification, Store, Urgency};

#[derive(Clone)]
struct App {
    store: Arc<Store>,
    tx: flume::Sender<Cmd>,
    token: Arc<str>,
    port: u16,
}

pub async fn serve(
    listener: std::net::TcpListener,
    store: Arc<Store>,
    tx: flume::Sender<Cmd>,
    token: String,
) -> anyhow::Result<()> {
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let app = App {
        store,
        tx,
        token: token.into(),
        port,
    };
    let router = Router::new()
        .route("/v1/state", get(state))
        .route("/v1/notifications", get(list).post(post).delete(clear))
        .route("/v1/notifications/{id}", delete(remove))
        .route("/v1/dnd", put(set_dnd))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app);
    axum::serve(listener, router).await?;
    Ok(())
}

async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    if !allowed(&app, req.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

fn allowed(app: &App, headers: &HeaderMap) -> bool {
    if headers.contains_key(header::ORIGIN) {
        return false;
    }
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let host_ok =
        host == format!("127.0.0.1:{}", app.port) || host == format!("localhost:{}", app.port);
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    host_ok && bearer == Some(&*app.token)
}

async fn state(State(app): State<App>) -> Json<Value> {
    let history = app.store.history();
    Json(json!({ "dnd": app.store.dnd(), "count": history.len() }))
}

async fn list(State(app): State<App>) -> Json<Value> {
    Json(json!(app.store.history()))
}

async fn post(State(app): State<App>, Json(new): Json<NewNotification>) -> Json<Value> {
    let n = app.store.post(new);
    // Do Not Disturb keeps the toast back; critical gets through.
    if !app.store.dnd() || n.urgency == Urgency::Critical {
        let _ = app.tx.send(Cmd::Toast(n.clone()));
    } else {
        let _ = app.tx.send(Cmd::Refresh);
    }
    Json(json!({ "id": n.id }))
}

async fn remove(State(app): State<App>, Path(id): Path<String>) -> StatusCode {
    if app.store.remove(&id) {
        let _ = app.tx.send(Cmd::Refresh);
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn clear(State(app): State<App>) -> StatusCode {
    app.store.clear();
    let _ = app.tx.send(Cmd::Refresh);
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct Dnd {
    enabled: bool,
}

async fn set_dnd(State(app): State<App>, Json(body): Json<Dnd>) -> StatusCode {
    app.store.set_dnd(body.enabled);
    let _ = app.tx.send(Cmd::Refresh);
    StatusCode::NO_CONTENT
}
