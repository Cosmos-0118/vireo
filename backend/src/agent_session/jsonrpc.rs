//! A small JSON-RPC 2.0 client over line-delimited stdio, shared by the Codex `app-server` and
//! Agent Client Protocol (ACP) drivers.
//!
//! One reader task owns the pipe: responses resolve the matching [`RpcClient::request`], and
//! everything else (server notifications and server-initiated requests) is forwarded on a channel.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    fn closed() -> Self {
        Self { code: -32000, message: "The agent process closed the connection.".to_string() }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

/// A message from the server that is not a response to one of our requests.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Notification { method: String, params: Value },
    /// The server expects an answer via [`RpcClient::respond`] / [`RpcClient::respond_error`].
    /// `id` keeps its original JSON type (number or string).
    Request { id: Value, method: String, params: Value },
    /// The server closed its end of the pipe.
    Closed,
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, RpcError>>>>>;

#[derive(Clone)]
pub struct RpcClient {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    /// ACP requires `"jsonrpc":"2.0"` on every message; Codex omits it.
    version_field: bool,
}

impl RpcClient {
    /// Starts the reader and writer tasks over `reader` / `writer`.
    pub fn spawn<R, W>(reader: R, writer: W, version_field: bool) -> (Self, mpsc::UnboundedReceiver<Incoming>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<Incoming>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        let mut writer = writer;
        tokio::spawn(async move {
            while let Some(mut line) = out_rx.recv().await {
                line.push('\n');
                if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
        });

        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                dispatch(&v, &reader_pending, &in_tx);
            }
            // Pipe closed: fail every request still waiting, then tell the consumer.
            let waiting: Vec<_> = reader_pending.lock().map(|mut m| m.drain().collect()).unwrap_or_default();
            for (_, tx) in waiting {
                let _ = tx.send(Err(RpcError::closed()));
            }
            let _ = in_tx.send(Incoming::Closed);
        });

        let client = Self { out: out_tx, pending, next_id: Arc::new(AtomicU64::new(1)), version_field };
        (client, in_rx)
    }

    fn envelope(&self, mut body: Value) -> String {
        if self.version_field {
            body["jsonrpc"] = json!("2.0");
        }
        body.to_string()
    }

    /// Sends a request and waits for its response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut m) = self.pending.lock() {
            m.insert(id.to_string(), tx);
        }
        let line = self.envelope(json!({ "id": id, "method": method, "params": params }));
        if self.out.send(line).is_err() {
            self.forget(id);
            return Err(RpcError::closed());
        }
        rx.await.unwrap_or_else(|_| Err(RpcError::closed()))
    }

    fn forget(&self, id: u64) {
        if let Ok(mut m) = self.pending.lock() {
            m.remove(&id.to_string());
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        let body = if params.is_null() { json!({ "method": method }) } else { json!({ "method": method, "params": params }) };
        let _ = self.out.send(self.envelope(body));
    }

    pub fn respond(&self, id: &Value, result: Value) {
        let _ = self.out.send(self.envelope(json!({ "id": id, "result": result })));
    }

    pub fn respond_error(&self, id: &Value, code: i64, message: &str) {
        let _ = self.out.send(self.envelope(json!({ "id": id, "error": { "code": code, "message": message } })));
    }
}

fn dispatch(v: &Value, pending: &Pending, in_tx: &mpsc::UnboundedSender<Incoming>) {
    let id = v.get("id").filter(|i| !i.is_null());
    match (v["method"].as_str(), id) {
        (Some(method), Some(id)) => {
            let _ = in_tx.send(Incoming::Request {
                id: id.clone(),
                method: method.to_string(),
                params: v["params"].clone(),
            });
        }
        (Some(method), None) => {
            let _ = in_tx.send(Incoming::Notification { method: method.to_string(), params: v["params"].clone() });
        }
        (None, Some(id)) => {
            let key = match id {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let tx = pending.lock().ok().and_then(|mut m| m.remove(&key));
            let Some(tx) = tx else { return };
            let result = match v.get("error").filter(|e| !e.is_null()) {
                Some(e) => Err(RpcError {
                    code: e["code"].as_i64().unwrap_or(-32603),
                    message: e["message"].as_str().unwrap_or("Unknown error").to_string(),
                }),
                None => Ok(v["result"].clone()),
            };
            let _ = tx.send(result);
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, split};

    /// Wires a client to an in-memory "server" end; returns the server's reader and writer.
    fn pair(version_field: bool) -> (RpcClient, mpsc::UnboundedReceiver<Incoming>, impl AsyncRead + Unpin, impl AsyncWrite + Unpin) {
        let (client_side, server_side) = duplex(8192);
        let (cr, cw) = split(client_side);
        let (sr, sw) = split(server_side);
        let (client, rx) = RpcClient::spawn(cr, cw, version_field);
        (client, rx, sr, sw)
    }

    async fn read_json(lines: &mut tokio::io::Lines<BufReader<impl AsyncRead + Unpin>>) -> Value {
        serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn request_resolves_with_matching_response() {
        let (client, _rx, sr, mut sw) = pair(false);
        let mut lines = BufReader::new(sr).lines();
        let c2 = client.clone();
        let call = tokio::spawn(async move { c2.request("thread/start", json!({ "cwd": "/w" })).await });
        let req = read_json(&mut lines).await;
        assert_eq!(req["method"], "thread/start");
        assert!(req.get("jsonrpc").is_none());
        let reply = json!({ "id": req["id"], "result": { "ok": true } });
        sw.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        assert_eq!(call.await.unwrap().unwrap(), json!({ "ok": true }));
    }

    #[tokio::test]
    async fn version_field_and_error_responses() {
        let (client, _rx, sr, mut sw) = pair(true);
        let mut lines = BufReader::new(sr).lines();
        let c2 = client.clone();
        let call = tokio::spawn(async move { c2.request("session/new", json!({})).await });
        let req = read_json(&mut lines).await;
        assert_eq!(req["jsonrpc"], "2.0");
        let reply = json!({ "id": req["id"], "error": { "code": -32601, "message": "nope" } });
        sw.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!((err.code, err.message.as_str()), (-32601, "nope"));
    }

    #[tokio::test]
    async fn server_requests_keep_their_id_type_and_get_answers() {
        let (client, mut rx, sr, mut sw) = pair(false);
        let mut lines = BufReader::new(sr).lines();
        sw.write_all(b"{\"id\":0,\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"a\":1}}\n")
            .await
            .unwrap();
        sw.write_all(b"{\"id\":\"abc\",\"method\":\"session/request_permission\",\"params\":{}}\n").await.unwrap();
        sw.write_all(b"{\"method\":\"turn/started\",\"params\":{\"t\":1}}\n").await.unwrap();
        let Some(Incoming::Request { id, method, .. }) = rx.recv().await else { panic!("want request") };
        assert_eq!((id.clone(), method.as_str()), (json!(0), "item/commandExecution/requestApproval"));
        client.respond(&id, json!({ "decision": "accept" }));
        let Some(Incoming::Request { id: id2, .. }) = rx.recv().await else { panic!("want request") };
        assert_eq!(id2, json!("abc"));
        client.respond_error(&id2, -32601, "unsupported");
        assert!(matches!(rx.recv().await, Some(Incoming::Notification { .. })));
        let first = read_json(&mut lines).await;
        assert_eq!(first, json!({ "id": 0, "result": { "decision": "accept" } }));
        let second = read_json(&mut lines).await;
        assert_eq!(second["id"], "abc");
        assert_eq!(second["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn closing_the_pipe_fails_waiting_requests() {
        let (client, mut rx, sr, sw) = pair(false);
        let mut lines = BufReader::new(sr).lines();
        let c2 = client.clone();
        let call = tokio::spawn(async move { c2.request("turn/start", json!({})).await });
        let _ = read_json(&mut lines).await;
        drop(sw);
        drop(lines);
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, -32000);
        assert_eq!(rx.recv().await, Some(Incoming::Closed));
    }
}
