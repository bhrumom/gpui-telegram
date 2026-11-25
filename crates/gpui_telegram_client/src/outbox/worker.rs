use crate::cache::LocalStore;
use crate::net::{HttpClient, HttpClientError, RichSendMessageRequest, RichSendMessageResponse};
use crate::outbox::network::NetworkStatusListener;
use crate::outbox::store::{OutboxMessageRecord, OutboxStatus, OutboxStore};
use crate::sync::ClientSyncCoordinator;
use gpui_telegram_core::error::Result;
use gpui_telegram_core::types::{ChatId, MessageId};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify};

pub const DEFAULT_OUTBOX_MAX_RETRIES: u32 = 10;

pub fn compute_base_backoff_ms(retry_count: u32) -> u64 {
    let exp = retry_count.min(6);
    500u64.saturating_mul(1u64 << exp).min(30_000)
}

pub fn compute_jittered_backoff_ms(retry_count: u32) -> u64 {
    let base = compute_base_backoff_ms(retry_count);
    let mut rand_bytes = [0u8; 4];
    let _ = getrandom::getrandom(&mut rand_bytes);
    let unit = u32::from_le_bytes(rand_bytes) as f64 / u32::MAX as f64;
    let factor = 0.75 + (unit * 0.50);
    let jittered = ((base as f64) * factor).round() as u64;
    jittered.clamp(250, 30_000)
}

#[derive(Debug, Clone)]
pub struct OutboxStatusEvent {
    pub client_nonce: u64,
    pub chat_id: i64,
    pub status: OutboxStatus,
    pub server_message_id: Option<i64>,
    pub retry_count: u32,
    pub last_error: Option<String>,
}

enum FailureClass {
    Transient(String),
    Permanent(String),
}

fn classify_http_error(err: &HttpClientError) -> FailureClass {
    match err {
        HttpClientError::Conflict(msg) => FailureClass::Permanent(format!("409 Conflict: {msg}")),
        HttpClientError::Api { status, message } => match *status {
            400 | 403 | 404 | 409 | 422 => {
                FailureClass::Permanent(format!("HTTP {status}: {message}"))
            }
            _ => FailureClass::Transient(format!("HTTP {status}: {message}")),
        },
        HttpClientError::Unauthorized(msg) => {
            FailureClass::Transient(format!("Unauthorized: {msg}"))
        }
        HttpClientError::Serialization(e) => {
            FailureClass::Permanent(format!("Serialization error: {e}"))
        }
        HttpClientError::Network(e) => FailureClass::Transient(format!("Network error: {e}")),
        HttpClientError::Proxy(e) => FailureClass::Transient(format!("Proxy error: {e}")),
    }
}

pub type TokenProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct OutboxWorker {
    store: OutboxStore,
    network: Arc<NetworkStatusListener>,
    http_client: HttpClient,
    token_provider: TokenProvider,
    local_store: LocalStore,
    sync_coordinator: ClientSyncCoordinator,
    wake_notify: Arc<Notify>,
    flush_mutex: AsyncMutex<()>,
    max_retries: u32,
    is_running: Arc<RwLock<bool>>,
    session_epoch: Arc<AtomicU64>,
    worker_gen: Arc<AtomicU64>,
    events_tx: broadcast::Sender<OutboxStatusEvent>,
}

impl std::fmt::Debug for OutboxWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxWorker")
            .field("max_retries", &self.max_retries)
            .field("is_running", &*self.is_running.read())
            .finish()
    }
}

impl OutboxWorker {
    pub fn new(
        store: OutboxStore,
        network: Arc<NetworkStatusListener>,
        http_client: HttpClient,
        token_provider: TokenProvider,
        local_store: LocalStore,
        sync_coordinator: ClientSyncCoordinator,
    ) -> Arc<Self> {
        Self::new_with_session_epoch(
            store,
            network,
            http_client,
            token_provider,
            local_store,
            sync_coordinator,
            Arc::new(AtomicU64::new(0)),
        )
    }

    pub fn new_with_session_epoch(
        store: OutboxStore,
        network: Arc<NetworkStatusListener>,
        http_client: HttpClient,
        token_provider: TokenProvider,
        local_store: LocalStore,
        sync_coordinator: ClientSyncCoordinator,
        session_epoch: Arc<AtomicU64>,
    ) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            store,
            network,
            http_client,
            token_provider,
            local_store,
            sync_coordinator,
            wake_notify: Arc::new(Notify::new()),
            flush_mutex: AsyncMutex::new(()),
            max_retries: DEFAULT_OUTBOX_MAX_RETRIES,
            is_running: Arc::new(RwLock::new(false)),
            session_epoch,
            worker_gen: Arc::new(AtomicU64::new(0)),
            events_tx,
        })
    }

    pub fn session_epoch(&self) -> u64 {
        self.session_epoch.load(Ordering::SeqCst)
    }

    pub fn bump_session_epoch(&self) -> u64 {
        self.session_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn clear_session_state(&self) {
        self.stop();
        self.bump_session_epoch();
        self.store.clear();
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries.max(1);
        self
    }

    pub fn store(&self) -> &OutboxStore {
        &self.store
    }

    pub fn network(&self) -> &Arc<NetworkStatusListener> {
        &self.network
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<OutboxStatusEvent> {
        self.events_tx.subscribe()
    }

    pub fn wake(&self) {
        self.wake_notify.notify_one();
    }

    pub fn enqueue(&self, req: &RichSendMessageRequest, nonce: u64) -> Result<OutboxMessageRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let rec = self.store.enqueue(req, nonce, now_ms)?;
        let _ = self.events_tx.send(OutboxStatusEvent {
            client_nonce: rec.client_nonce,
            chat_id: rec.chat_id,
            status: rec.status,
            server_message_id: rec.server_message_id,
            retry_count: rec.retry_count,
            last_error: rec.last_error.clone(),
        });
        self.wake();
        Ok(rec)
    }

    pub fn recover_in_flight(&self) -> Result<usize> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.store.recover_in_flight(now_ms)
    }

    pub fn start(self: &Arc<Self>, handle: &Handle) {
        let mut running = self.is_running.write();
        if *running {
            return;
        }
        *running = true;
        let my_gen = self.worker_gen.fetch_add(1, Ordering::SeqCst) + 1;
        drop(running);

        let _ = self.recover_in_flight();

        let worker = Arc::clone(self);
        let reconnect_notify = self.network.reconnect_notify();
        let mut epoch_rx = self.network.subscribe_reconnect_epoch();
        let mut last_seen_epoch = self.network.reconnect_epoch();
        let wake_notify = Arc::clone(&self.wake_notify);
        let is_running = Arc::clone(&self.is_running);
        let worker_gen = Arc::clone(&self.worker_gen);

        handle.spawn(async move {
            while *is_running.read() && worker_gen.load(Ordering::SeqCst) == my_gen {
                let pre_flush_epoch = worker.network.reconnect_epoch();
                if pre_flush_epoch != last_seen_epoch {
                    last_seen_epoch = pre_flush_epoch;
                    let _ = *epoch_rx.borrow_and_update();
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let _ = worker.store.reset_all_pending_retry_times(now_ms);
                }

                if worker.network.is_online() {
                    let _ = worker.flush_due().await;
                }
                if !*is_running.read() || worker_gen.load(Ordering::SeqCst) != my_gen {
                    break;
                }

                let post_flush_epoch = worker.network.reconnect_epoch();
                if post_flush_epoch != last_seen_epoch {
                    last_seen_epoch = post_flush_epoch;
                    let _ = *epoch_rx.borrow_and_update();
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let _ = worker.store.reset_all_pending_retry_times(now_ms);
                    continue;
                }

                tokio::select! {
                    res = epoch_rx.changed() => {
                        if res.is_ok() {
                            let new_epoch = *epoch_rx.borrow_and_update();
                            if new_epoch != last_seen_epoch {
                                last_seen_epoch = new_epoch;
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let _ = worker.store.reset_all_pending_retry_times(now_ms);
                            }
                        }
                    }
                    _ = reconnect_notify.notified() => {
                        last_seen_epoch = worker.network.reconnect_epoch();
                        let _ = *epoch_rx.borrow_and_update();
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let _ = worker.store.reset_all_pending_retry_times(now_ms);
                    }
                    _ = wake_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                }
            }
        });
    }

    pub fn stop(&self) {
        *self.is_running.write() = false;
        self.worker_gen.fetch_add(1, Ordering::SeqCst);
        self.wake_notify.notify_waiters();
    }

    pub fn notify_reconnected_and_flush(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<OutboxMessageRecord>>> + Send + '_ {
        let start_epoch = self.session_epoch.load(Ordering::SeqCst);
        async move {
            let actual = self.session_epoch.load(Ordering::SeqCst);
            if actual != start_epoch {
                return Err(gpui_telegram_core::error::TelegramError::GenerationFenced {
                    expected: start_epoch,
                    actual,
                });
            }
            let _flush_guard = self.flush_mutex.lock().await;
            let actual = self.session_epoch.load(Ordering::SeqCst);
            if actual != start_epoch {
                return Err(gpui_telegram_core::error::TelegramError::GenerationFenced {
                    expected: start_epoch,
                    actual,
                });
            }
            let now_ms = chrono::Utc::now().timestamp_millis();
            self.network.notify_reconnected();
            let actual = self.session_epoch.load(Ordering::SeqCst);
            if actual != start_epoch {
                return Err(gpui_telegram_core::error::TelegramError::GenerationFenced {
                    expected: start_epoch,
                    actual,
                });
            }
            self.store.reset_all_pending_retry_times(now_ms)?;
            self.flush_due_locked(start_epoch).await
        }
    }

    pub fn flush_due(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<OutboxMessageRecord>>> + Send + '_ {
        let start_epoch = self.session_epoch.load(Ordering::SeqCst);
        async move {
            let actual = self.session_epoch.load(Ordering::SeqCst);
            if actual != start_epoch {
                return Err(gpui_telegram_core::error::TelegramError::GenerationFenced {
                    expected: start_epoch,
                    actual,
                });
            }
            let _flush_guard = self.flush_mutex.lock().await;
            let actual = self.session_epoch.load(Ordering::SeqCst);
            if actual != start_epoch {
                return Err(gpui_telegram_core::error::TelegramError::GenerationFenced {
                    expected: start_epoch,
                    actual,
                });
            }
            self.flush_due_locked(start_epoch).await
        }
    }

    async fn flush_due_locked(&self, flush_epoch: u64) -> Result<Vec<OutboxMessageRecord>> {
        let mut processed = Vec::new();
        while self.network.is_online() && self.session_epoch.load(Ordering::SeqCst) == flush_epoch {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let Some(claimed) = self.store.claim_next_due(now_ms)? else {
                break;
            };
            if self.session_epoch.load(Ordering::SeqCst) != flush_epoch {
                break;
            }
            let outcome = self.dispatch_claimed(&claimed, flush_epoch).await;
            processed.push(outcome);
        }
        Ok(processed)
    }

    pub fn send_or_enqueue_direct<'a>(
        &'a self,
        req: &'a RichSendMessageRequest,
        nonce: u64,
    ) -> impl std::future::Future<Output = std::result::Result<RichSendMessageResponse, HttpClientError>>
           + Send
           + 'a {
        let start_epoch = self.session_epoch.load(Ordering::SeqCst);
        let start_token = (self.token_provider)();
        async move {
            if self.session_epoch.load(Ordering::SeqCst) != start_epoch {
                return Err(HttpClientError::Unauthorized("Session changed".to_string()));
            }
            let now_ms = chrono::Utc::now().timestamp_millis();
            if let Ok(Some(existing)) = self.store.get(nonce) {
                return Err(HttpClientError::Conflict(format!(
                    "Duplicate client_nonce {nonce}: already in outbox ({})",
                    existing.status.as_str()
                )));
            }
            if !self.network.is_online() {
                if self.session_epoch.load(Ordering::SeqCst) != start_epoch {
                    return Err(HttpClientError::Unauthorized("Session changed".to_string()));
                }
                self.enqueue(req, nonce).map_err(|e| HttpClientError::Api {
                    status: 400,
                    message: e.to_string(),
                })?;
                return Err(HttpClientError::Api {
                    status: 503,
                    message: "Client is offline; message queued in persistent outbox".into(),
                });
            }
            let Some(token) = start_token.filter(|t| !t.trim().is_empty()) else {
                if self.session_epoch.load(Ordering::SeqCst) != start_epoch {
                    return Err(HttpClientError::Unauthorized(
                        "No active session token".to_string(),
                    ));
                }
                self.enqueue(req, nonce).map_err(|e| HttpClientError::Api {
                    status: 400,
                    message: e.to_string(),
                })?;
                return Err(HttpClientError::Unauthorized(
                    "No active session token".to_string(),
                ));
            };
            if self.session_epoch.load(Ordering::SeqCst) != start_epoch {
                return Err(HttpClientError::Unauthorized("Session changed".to_string()));
            }
            if let Err(e) = self.store.enqueue_in_flight(req, nonce, now_ms) {
                return Err(HttpClientError::Api {
                    status: 400,
                    message: e.to_string(),
                });
            }
            let res = self.http_client.send_message_rich(&token, req).await;
            if self.session_epoch.load(Ordering::SeqCst) != start_epoch {
                return res;
            }
            match res {
                Ok(resp) => {
                    let done_ms = chrono::Utc::now().timestamp_millis();
                    if let Ok(updated) = self.store.mark_delivered(nonce, resp.message_id, done_ms)
                    {
                        self.emit_event(&updated);
                    }
                    self.apply_delivery_ack(nonce, &resp);
                    Ok(resp)
                }
                Err(err) => {
                    let fail_ms = chrono::Utc::now().timestamp_millis();
                    let _ = self.record_dispatch_error(nonce, 0, &err, fail_ms);
                    Err(err)
                }
            }
        }
    }

    pub async fn send_or_enqueue_direct_with_token(
        &self,
        token: &str,
        expected_epoch: u64,
        req: &RichSendMessageRequest,
        nonce: u64,
    ) -> std::result::Result<RichSendMessageResponse, HttpClientError> {
        if token.trim().is_empty() || self.session_epoch.load(Ordering::SeqCst) != expected_epoch {
            return Err(HttpClientError::Unauthorized(
                "Session changed before pinned send".to_string(),
            ));
        }
        if (self.token_provider)().is_some_and(|cur| !cur.trim().is_empty() && cur != token) {
            return Err(HttpClientError::Unauthorized(
                "Session token changed before pinned send".to_string(),
            ));
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Ok(Some(existing)) = self.store.get(nonce) {
            return Err(HttpClientError::Conflict(format!(
                "Duplicate client_nonce {nonce}: already in outbox ({})",
                existing.status.as_str()
            )));
        }
        if !self.network.is_online() {
            if self.session_epoch.load(Ordering::SeqCst) != expected_epoch {
                return Err(HttpClientError::Unauthorized("Session changed".to_string()));
            }
            self.enqueue(req, nonce).map_err(|e| HttpClientError::Api {
                status: 400,
                message: e.to_string(),
            })?;
            return Err(HttpClientError::Api {
                status: 503,
                message: "Client is offline; message queued in persistent outbox".into(),
            });
        }
        if self.session_epoch.load(Ordering::SeqCst) != expected_epoch {
            return Err(HttpClientError::Unauthorized("Session changed".to_string()));
        }
        if let Err(e) = self.store.enqueue_in_flight(req, nonce, now_ms) {
            return Err(HttpClientError::Api {
                status: 400,
                message: e.to_string(),
            });
        }
        let res = self.http_client.send_message_rich(token, req).await;
        if self.session_epoch.load(Ordering::SeqCst) != expected_epoch {
            return res;
        }
        match res {
            Ok(resp) => {
                let done_ms = chrono::Utc::now().timestamp_millis();
                if let Ok(updated) = self.store.mark_delivered(nonce, resp.message_id, done_ms) {
                    self.emit_event(&updated);
                }
                self.apply_delivery_ack(nonce, &resp);
                Ok(resp)
            }
            Err(err) => {
                let fail_ms = chrono::Utc::now().timestamp_millis();
                let _ = self.record_dispatch_error(nonce, 0, &err, fail_ms);
                Err(err)
            }
        }
    }

    async fn dispatch_claimed(
        &self,
        record: &OutboxMessageRecord,
        claim_epoch: u64,
    ) -> OutboxMessageRecord {
        if self.session_epoch.load(Ordering::SeqCst) != claim_epoch {
            return record.clone();
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let req = match record.decode_request() {
            Ok(r) => r,
            Err(e) => {
                if self.session_epoch.load(Ordering::SeqCst) != claim_epoch {
                    return record.clone();
                }
                let updated = self
                    .store
                    .mark_failed(record.client_nonce, &e.to_string(), now_ms)
                    .unwrap_or_else(|_| record.clone());
                self.emit_event(&updated);
                return updated;
            }
        };

        if self.session_epoch.load(Ordering::SeqCst) != claim_epoch {
            return record.clone();
        }
        let Some(token) = (self.token_provider)().filter(|t| !t.trim().is_empty()) else {
            if self.session_epoch.load(Ordering::SeqCst) != claim_epoch {
                return record.clone();
            }
            let next_retry_count = record.retry_count.saturating_add(1);
            let updated = if next_retry_count >= self.max_retries {
                self.store
                    .mark_failed(
                        record.client_nonce,
                        &format!("Max retries ({}) exceeded: Missing auth token", self.max_retries),
                        now_ms,
                    )
                    .unwrap_or_else(|_| record.clone())
            } else {
                let next_retry = now_ms + compute_jittered_backoff_ms(record.retry_count) as i64;
                self.store
                    .schedule_retry(
                        record.client_nonce,
                        next_retry_count,
                        next_retry,
                        "Missing auth token",
                        now_ms,
                    )
                    .unwrap_or_else(|_| record.clone())
            };
            self.emit_event(&updated);
            return updated;
        };

        if self.session_epoch.load(Ordering::SeqCst) != claim_epoch {
            return record.clone();
        }
        let res = self.http_client.send_message_rich(&token, &req).await;
        if self.session_epoch.load(Ordering::SeqCst) != claim_epoch {
            return record.clone();
        }
        match res {
            Ok(resp) => {
                let done_ms = chrono::Utc::now().timestamp_millis();
                let updated = self
                    .store
                    .mark_delivered(record.client_nonce, resp.message_id, done_ms)
                    .unwrap_or_else(|_| record.clone());
                self.apply_delivery_ack(record.client_nonce, &resp);
                self.emit_event(&updated);
                updated
            }
            Err(err) => {
                let fail_ms = chrono::Utc::now().timestamp_millis();
                self.record_dispatch_error(record.client_nonce, record.retry_count, &err, fail_ms)
                    .unwrap_or_else(|_| record.clone())
            }
        }
    }

    fn record_dispatch_error(
        &self,
        client_nonce: u64,
        current_retry_count: u32,
        err: &HttpClientError,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        let updated = match classify_http_error(err) {
            FailureClass::Permanent(msg) => self.store.mark_failed(client_nonce, &msg, now_ms)?,
            FailureClass::Transient(msg) => {
                let next_retry_count = current_retry_count.saturating_add(1);
                if next_retry_count >= self.max_retries {
                    self.store.mark_failed(
                        client_nonce,
                        &format!("Max retries ({}) exceeded: {msg}", self.max_retries),
                        now_ms,
                    )?
                } else {
                    let delay_ms = compute_jittered_backoff_ms(current_retry_count);
                    let next_retry_at_ms = now_ms.saturating_add(delay_ms as i64);
                    self.store.schedule_retry(
                        client_nonce,
                        next_retry_count,
                        next_retry_at_ms,
                        &msg,
                        now_ms,
                    )?
                }
            }
        };
        self.emit_event(&updated);
        Ok(updated)
    }

    fn apply_delivery_ack(&self, client_nonce: u64, resp: &RichSendMessageResponse) {
        self.local_store
            .ack_client_nonce(client_nonce, MessageId(resp.message_id));
        let cid = ChatId(resp.chat_id);
        if resp.pts > self.sync_coordinator.last_pts(cid) {
            self.sync_coordinator.set_pts(cid, resp.pts);
        }
    }

    fn emit_event(&self, rec: &OutboxMessageRecord) {
        let _ = self.events_tx.send(OutboxStatusEvent {
            client_nonce: rec.client_nonce,
            chat_id: rec.chat_id,
            status: rec.status,
            server_message_id: rec.server_message_id,
            retry_count: rec.retry_count,
            last_error: rec.last_error.clone(),
        });
    }
}
