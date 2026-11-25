//! Client side of the endpoint, used by `tmx` and tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::clients::{ClientKind, Scope};
use crate::error::ControlError;
use crate::frame::{read_frame, write_frame};
use crate::paths::Endpoint;
use crate::protocol::{
    API_VERSION, HelloParams, HelloResult, Notification, PairParams, PairResult, Response,
};
use crate::token::Token;

type Waiters = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A connection to a control endpoint.
pub struct ControlClient {
    out: mpsc::Sender<Vec<u8>>,
    waiters: Waiters,
    next_id: AtomicU64,
    notifications: mpsc::UnboundedReceiver<Notification>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for ControlClient {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl ControlClient {
    /// Connect (Unix: after checking that the socket belongs to this user).
    pub async fn connect(endpoint: &Endpoint) -> Result<Self, ControlError> {
        let stream = crate::transport::connect(endpoint).await?;
        let (mut rd, mut wr) = tokio::io::split(stream);
        let (out, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
        let writer = tokio::spawn(async move {
            while let Some(body) = out_rx.recv().await {
                if write_frame(&mut wr, &body).await.is_err() {
                    break;
                }
            }
        });
        let waiters: Waiters = Arc::default();
        let (note_tx, notifications) = mpsc::unbounded_channel();
        let w = Arc::clone(&waiters);
        let reader = tokio::spawn(async move {
            while let Ok(Some(frame)) = read_frame(&mut rd).await {
                let Ok(v) = serde_json::from_slice::<Value>(&frame) else {
                    tracing::debug!("control client: ignoring a malformed frame");
                    continue;
                };
                if v.get("method").is_some() {
                    if let Ok(n) = serde_json::from_value::<Notification>(v) {
                        let _ = note_tx.send(n);
                    }
                    continue;
                }
                let Ok(resp) = serde_json::from_value::<Response>(v) else {
                    continue;
                };
                if let Some(id) = resp.id.as_u64()
                    && let Some(tx) = lock(&w).remove(&id)
                {
                    let _ = tx.send(resp);
                }
            }
            // Wake every waiter with `Closed`.
            lock(&w).clear();
        });
        Ok(Self {
            out,
            waiters,
            next_id: AtomicU64::new(1),
            notifications,
            tasks: vec![writer, reader],
        })
    }

    /// Call `method` and return its raw result.
    pub async fn call_value(&self, method: &str, params: Value) -> Result<Value, ControlError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.waiters).insert(id, tx);
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))
        .map_err(|e| ControlError::Protocol(e.to_string()))?;
        if self.out.send(body).await.is_err() {
            lock(&self.waiters).remove(&id);
            return Err(ControlError::Closed);
        }
        let resp = rx.await.map_err(|_| ControlError::Closed)?;
        match (resp.result, resp.error) {
            (_, Some(e)) => Err(ControlError::Rpc {
                rpc: e.code,
                code: e.ctl_code(),
                message: e.message,
                data: e.data,
            }),
            (Some(v), None) => Ok(v),
            (None, None) => Err(ControlError::Protocol("response without result".into())),
        }
    }

    /// Call `method` with typed parameters and result.
    pub async fn call<P: Serialize, T: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<T, ControlError> {
        let params =
            serde_json::to_value(params).map_err(|e| ControlError::Protocol(e.to_string()))?;
        let v = self.call_value(method, params).await?;
        serde_json::from_value(v).map_err(|e| ControlError::Protocol(e.to_string()))
    }

    /// Authenticate this connection with a stored token.
    pub async fn hello(&self, token: Option<Token>) -> Result<HelloResult, ControlError> {
        self.call(
            "control.hello",
            &HelloParams {
                api_version: API_VERSION,
                token,
            },
        )
        .await
    }

    /// Ask to pair; waits until the user decides in Termax.
    pub async fn pair(
        &self,
        name: &str,
        kind: ClientKind,
        scopes: Vec<Scope>,
    ) -> Result<PairResult, ControlError> {
        self.call(
            "control.pair",
            &PairParams {
                api_version: API_VERSION,
                name: name.to_owned(),
                kind,
                scopes,
            },
        )
        .await
    }

    /// The next stream notification (`None` once the connection closed).
    pub async fn next_notification(&mut self) -> Option<Notification> {
        self.notifications.recv().await
    }
}
