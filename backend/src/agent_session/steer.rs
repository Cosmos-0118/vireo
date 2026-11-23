//! Mid-turn agent steering: thread-indexed active turn registry, atomic turn-end handshake,
//! and HTTP endpoints (`POST /api/agent/steer`, `POST /api/agent/steer/cancel`).

use super::types::SseTx;
use crate::agent::send_sse;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Notify};

const CANCEL_HOOK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerMode {
    #[default]
    Steer,
    Now,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerDelivery {
    MidTurn,
    Continuation,
    Preempted,
}

impl SteerDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MidTurn => "mid_turn",
            Self::Continuation => "continuation",
            Self::Preempted => "preempted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerItem {
    pub steer_id: String,
    pub text: String,
    pub mode: SteerMode,
}

#[derive(Default)]
struct TurnSteerState {
    closed: bool,
    queue: VecDeque<SteerItem>,
}

type CancelHookTx = mpsc::UnboundedSender<(String, oneshot::Sender<bool>)>;

/// Live steering channel for one active chat turn (CLI session or direct API loop).
pub struct ActiveTurnEntry {
    pub thread_id: String,
    pub turn_id: String,
    state: Mutex<TurnSteerState>,
    notify: Notify,
    cancel_hook: Mutex<Option<CancelHookTx>>,
}

impl ActiveTurnEntry {
    pub fn new(thread_id: String, turn_id: String) -> Self {
        Self {
            thread_id,
            turn_id,
            state: Mutex::new(TurnSteerState::default()),
            notify: Notify::new(),
            cancel_hook: Mutex::new(None),
        }
    }

    /// Enqueues a steer prompt unless the turn has already closed atomically.
    pub fn enqueue(&self, text: String, mode: SteerMode) -> Result<SteerItem, &'static str> {
        let mut s = self.state.lock().map_err(|_| "turn_ended")?;
        if s.closed {
            return Err("turn_ended");
        }
        let item = SteerItem {
            steer_id: uuid::Uuid::new_v4().to_string(),
            text,
            mode,
        };
        s.queue.push_back(item.clone());
        drop(s);
        self.notify.notify_one();
        Ok(item)
    }

    /// Puts a steer item back at the front of the queue (e.g. non-steerable Codex turn).
    pub fn requeue_front(&self, item: SteerItem) {
        if let Ok(mut s) = self.state.lock() {
            if !s.closed {
                s.queue.push_front(item);
                drop(s);
                self.notify.notify_one();
            }
        }
    }

    #[cfg(test)]
    fn has_queued(&self) -> bool {
        self.state.lock().map(|s| !s.queue.is_empty()).unwrap_or(false)
    }

    pub fn has_now_pending(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.queue.iter().any(|i| i.mode == SteerMode::Now))
            .unwrap_or(false)
    }

    pub fn try_pop(&self) -> Option<SteerItem> {
        self.state.lock().ok()?.queue.pop_front()
    }

    /// Pops the earliest `Now` steer if one exists, otherwise the front of the queue.
    pub fn try_pop_now_or_front(&self) -> Option<SteerItem> {
        let mut s = self.state.lock().ok()?;
        if let Some(pos) = s.queue.iter().position(|i| i.mode == SteerMode::Now) {
            s.queue.remove(pos)
        } else {
            s.queue.pop_front()
        }
    }

    pub fn try_pop_all(&self) -> Vec<SteerItem> {
        self.state
            .lock()
            .map(|mut s| s.queue.drain(..).collect())
            .unwrap_or_default()
    }

    /// Atomic turn-end handshake (single-item drivers): if a steer is queued, returns it so the
    /// driver chains it; otherwise marks the turn closed under the same lock and returns `None`.
    pub fn try_close_if_empty(&self) -> Option<SteerItem> {
        let mut s = self.state.lock().ok()?;
        if let Some(item) = s.queue.pop_front() {
            return Some(item);
        }
        s.closed = true;
        None
    }

    /// Atomic turn-end handshake (batch drivers): if steers are queued, drains them all; otherwise
    /// marks the turn closed under the same lock and returns `None`.
    pub fn try_close_drain_all(&self) -> Option<Vec<SteerItem>> {
        let mut s = self.state.lock().ok()?;
        if !s.queue.is_empty() {
            return Some(s.queue.drain(..).collect());
        }
        s.closed = true;
        None
    }

    /// Marks the turn closed and returns any unconsumed steers so the caller can cancel them.
    pub fn force_close_and_drain(&self) -> Vec<SteerItem> {
        self.clear_cancel_hook();
        self.state
            .lock()
            .map(|mut s| {
                s.closed = true;
                s.queue.drain(..).collect()
            })
            .unwrap_or_default()
    }

    /// Removes a still-queued steer item before the engine consumes it.
    pub fn remove_queued(&self, steer_id: &str) -> bool {
        let Ok(mut s) = self.state.lock() else { return false };
        if let Some(pos) = s.queue.iter().position(|i| i.steer_id == steer_id) {
            s.queue.remove(pos);
            return true;
        }
        false
    }

    pub fn set_cancel_hook(&self, tx: CancelHookTx) {
        if let Ok(mut h) = self.cancel_hook.lock() {
            *h = Some(tx);
        }
    }

    pub fn clear_cancel_hook(&self) {
        if let Ok(mut h) = self.cancel_hook.lock() {
            *h = None;
        }
    }

    /// Cancels a steer item either locally (if still queued) or through the driver's async cancel
    /// hook (e.g. Claude `cancel_async_message`).
    pub async fn cancel(&self, steer_id: &str) -> bool {
        if self.remove_queued(steer_id) {
            return true;
        }
        let hook = self.cancel_hook.lock().ok().and_then(|h| h.clone());
        let Some(hook) = hook else { return false };
        let (tx, rx) = oneshot::channel();
        if hook.send((steer_id.to_string(), tx)).is_err() {
            return false;
        }
        tokio::time::timeout(CANCEL_HOOK_TIMEOUT, rx)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    }

    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

/// Thread-indexed registry of currently running turns.
#[derive(Default)]
pub struct ActiveTurnRegistry {
    map: Mutex<HashMap<String, Arc<ActiveTurnEntry>>>,
}

impl ActiveTurnRegistry {
    pub fn register(self: &Arc<Self>, thread_id: &str, turn_id: &str) -> ActiveTurnGuard {
        let entry = Arc::new(ActiveTurnEntry::new(thread_id.to_string(), turn_id.to_string()));
        if let Ok(mut m) = self.map.lock() {
            m.insert(thread_id.to_string(), entry.clone());
        }
        ActiveTurnGuard {
            registry: self.clone(),
            entry,
        }
    }

    pub fn get(&self, thread_id: &str) -> Option<Arc<ActiveTurnEntry>> {
        self.map.lock().ok()?.get(thread_id).cloned()
    }
}

/// Automatically closes and unregisters an active turn when the turn function exits.
pub struct ActiveTurnGuard {
    registry: Arc<ActiveTurnRegistry>,
    entry: Arc<ActiveTurnEntry>,
}

impl ActiveTurnGuard {
    pub fn entry(&self) -> &Arc<ActiveTurnEntry> {
        &self.entry
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        let _ = self.entry.force_close_and_drain();
        if let Ok(mut m) = self.registry.map.lock() {
            if m.get(&self.entry.thread_id)
                .is_some_and(|cur| cur.turn_id == self.entry.turn_id)
            {
                m.remove(&self.entry.thread_id);
            }
        }
    }
}

pub async fn emit_steer_applied(tx: &SseTx, item: &SteerItem, delivery: SteerDelivery) {
    send_sse(
        tx,
        "steer_applied",
        json!({
            "steer_id": item.steer_id,
            "text": item.text,
            "delivery": delivery.as_str(),
        }),
    )
    .await;
}

pub async fn emit_steer_cancelled(tx: &SseTx, steer_id: &str, reason: &str) {
    send_sse(
        tx,
        "steer_cancelled",
        json!({
            "steer_id": steer_id,
            "reason": reason,
        }),
    )
    .await;
}

#[derive(Deserialize)]
pub struct SteerReq {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub mode: SteerMode,
}

/// `POST /api/agent/steer`: injects or queues a mid-turn steering prompt for a running turn.
pub async fn steer_turn(
    State(state): State<AppState>,
    Json(req): Json<SteerReq>,
) -> (StatusCode, Json<Value>) {
    if req.text.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "accepted": false, "reason": "empty_text" })),
        );
    }
    let Some(entry) = state.active_turns.get(&req.thread_id) else {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "accepted": false, "reason": "turn_ended" })),
        );
    };
    if let Some(tid) = req.turn_id.as_deref().filter(|s| !s.is_empty()) {
        if tid != entry.turn_id {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "accepted": false, "reason": "turn_mismatch" })),
            );
        }
    }
    match entry.enqueue(req.text, req.mode) {
        Ok(item) => (
            StatusCode::OK,
            Json(json!({
                "accepted": true,
                "steer_id": item.steer_id,
                "turn_id": entry.turn_id,
            })),
        ),
        Err(reason) => (
            StatusCode::CONFLICT,
            Json(json!({ "accepted": false, "reason": reason })),
        ),
    }
}

#[derive(Deserialize)]
pub struct CancelSteerReq {
    pub thread_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub steer_id: String,
}

/// `POST /api/agent/steer/cancel`: withdraws a queued steer before the agent consumes it.
pub async fn cancel_steer(
    State(state): State<AppState>,
    Json(req): Json<CancelSteerReq>,
) -> Json<Value> {
    let Some(entry) = state.active_turns.get(&req.thread_id) else {
        return Json(json!({ "cancelled": false }));
    };
    if let Some(tid) = req.turn_id.as_deref().filter(|s| !s.is_empty()) {
        if tid != entry.turn_id {
            return Json(json!({ "cancelled": false }));
        }
    }
    let cancelled = entry.cancel(&req.steer_id).await;
    Json(json!({ "cancelled": cancelled }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_turn_end_handshake_prevents_lost_steers() {
        let reg = Arc::new(ActiveTurnRegistry::default());
        let guard = reg.register("t1", "turn-1");
        let entry = guard.entry().clone();

        let item = entry.enqueue("also fix tests".into(), SteerMode::Steer).unwrap();
        // Queue is non-empty, so try_close_if_empty yields the item and keeps the turn open.
        let popped = entry.try_close_if_empty().unwrap();
        assert_eq!(popped.steer_id, item.steer_id);

        // Turn is still open: another steer can be enqueued.
        let item2 = entry.enqueue("and format".into(), SteerMode::Now).unwrap();
        assert!(entry.has_now_pending());
        let popped2 = entry.try_pop_now_or_front().unwrap();
        assert_eq!(popped2.steer_id, item2.steer_id);

        // Now queue is empty: try_close_if_empty closes atomically.
        assert!(entry.try_close_if_empty().is_none());
        assert_eq!(entry.enqueue("too late".into(), SteerMode::Steer), Err("turn_ended"));
    }

    #[tokio::test]
    async fn cancel_removes_queued_or_delegates_to_hook() {
        let entry = ActiveTurnEntry::new("t1".into(), "turn-1".into());
        let s1 = entry.enqueue("first".into(), SteerMode::Steer).unwrap();
        assert!(entry.cancel(&s1.steer_id).await);
        assert!(!entry.has_queued());
        // Second cancel returns false.
        assert!(!entry.cancel(&s1.steer_id).await);

        // Dispatched item delegated to cancel hook.
        let (tx, mut rx) = mpsc::unbounded_channel();
        entry.set_cancel_hook(tx);
        let handle = tokio::spawn(async move {
            let (id, reply) = rx.recv().await.unwrap();
            let _ = reply.send(id == "dispatched-1");
        });
        assert!(entry.cancel("dispatched-1").await);
        handle.await.unwrap();
    }

    #[test]
    fn guard_drop_only_removes_matching_turn_id() {
        let reg = Arc::new(ActiveTurnRegistry::default());
        let g1 = reg.register("t1", "turn-1");
        let g2 = reg.register("t1", "turn-2");
        drop(g1);
        assert_eq!(reg.get("t1").map(|e| e.turn_id.clone()).as_deref(), Some("turn-2"));
        drop(g2);
        assert!(reg.get("t1").is_none());
    }
}
