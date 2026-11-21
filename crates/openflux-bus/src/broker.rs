//! In-process hierarchical pub/sub broker with retained state, TTL expiry, and origin loop prevention.

use crate::topic::{validate_pattern, validate_topic, TopicError, TopicPattern};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusMessage {
    pub seq: u64,
    pub topic: String,
    pub payload: Value,
    pub retain: bool,
    pub timestamp_ms: u64,
    /// Optional expiration timestamp in Unix ms (used for retained state TTL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    /// Origin node id (e.g. `"keyboard"`, `"host"`, `"module:pet"`) used by bus bridges to avoid echo loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl BusMessage {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_some_and(|exp| now_ms >= exp)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PublishOptions {
    pub retain: bool,
    pub ttl_ms: Option<u64>,
    pub origin: Option<String>,
    pub timestamp_ms: Option<u64>,
}

impl PublishOptions {
    pub fn retained() -> Self {
        Self {
            retain: true,
            ..Default::default()
        }
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }

    pub fn with_ttl_ms(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = Some(ttl_ms);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct SubscribeOptions {
    /// Deliver currently retained matching messages immediately upon subscribing.
    pub replay_retained: bool,
    /// Skip messages whose `origin` matches this value (used by bus bridges).
    pub ignore_origin: Option<String>,
    /// Maximum buffered messages before oldest are dropped to protect broker throughput.
    pub capacity: usize,
}

impl SubscribeOptions {
    pub fn with_retained() -> Self {
        Self {
            replay_retained: true,
            ignore_origin: None,
            capacity: 512,
        }
    }
}

struct SubQueue {
    id: u64,
    pattern: TopicPattern,
    ignore_origin: Option<String>,
    capacity: usize,
    queue: Mutex<VecDeque<BusMessage>>,
    cv: Condvar,
    dropped: AtomicU64,
}

impl SubQueue {
    fn push(&self, msg: BusMessage) {
        let mut q = self.queue.lock().unwrap();
        if q.len() >= self.capacity {
            q.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        q.push_back(msg);
        self.cv.notify_one();
    }
}

/// Active subscription handle. Automatically unsubscribes on drop.
pub struct Subscription {
    broker: Broker,
    sub: Arc<SubQueue>,
}

impl Subscription {
    pub fn id(&self) -> u64 {
        self.sub.id
    }

    pub fn pattern(&self) -> &str {
        self.sub.pattern.as_str()
    }

    pub fn dropped_count(&self) -> u64 {
        self.sub.dropped.load(Ordering::Relaxed)
    }

    /// Non-blocking drain of all currently queued messages.
    pub fn drain(&self) -> Vec<BusMessage> {
        let mut q = self.sub.queue.lock().unwrap();
        q.drain(..).collect()
    }

    /// Non-blocking receive of a single message.
    pub fn try_recv(&self) -> Option<BusMessage> {
        self.sub.queue.lock().unwrap().pop_front()
    }

    /// Blocking receive with timeout.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<BusMessage> {
        let mut q = self.sub.queue.lock().unwrap();
        if let Some(m) = q.pop_front() {
            return Some(m);
        }
        let (mut q, _) = self.sub.cv.wait_timeout(q, timeout).unwrap();
        q.pop_front()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.broker.unsubscribe(self.sub.id);
    }
}

struct BrokerInner {
    node_id: String,
    retained: BTreeMap<String, BusMessage>,
    history: VecDeque<BusMessage>,
    subscribers: Vec<Arc<SubQueue>>,
    next_sub_id: u64,
}

const MAX_BROKER_HISTORY: usize = 512;

/// Thread-safe, cheaply cloneable handle to an OpenFlux hierarchical pub/sub broker.
#[derive(Clone)]
pub struct Broker {
    inner: Arc<Mutex<BrokerInner>>,
    history_cv: Arc<Condvar>,
    seq: Arc<AtomicU64>,
}

impl Default for Broker {
    fn default() -> Self {
        Self::new("local")
    }
}

impl Broker {
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BrokerInner {
                node_id: node_id.into(),
                retained: BTreeMap::new(),
                history: VecDeque::with_capacity(MAX_BROKER_HISTORY),
                subscribers: Vec::new(),
                next_sub_id: 1,
            })),
            history_cv: Arc::new(Condvar::new()),
            seq: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn latest_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed).saturating_sub(1)
    }

    pub fn node_id(&self) -> String {
        self.inner.lock().unwrap().node_id.clone()
    }

    /// Publishes a JSON value onto a concrete hierarchical topic.
    pub fn publish(
        &self,
        topic: impl Into<String>,
        payload: Value,
        opts: PublishOptions,
    ) -> Result<BusMessage, TopicError> {
        let topic = topic.into();
        validate_topic(&topic)?;

        let ts = opts.timestamp_ms.unwrap_or_else(now_unix_ms);
        let expires_at_ms = opts.ttl_ms.map(|ttl| ts.saturating_add(ttl));
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);

        let (msg, matching) = {
            let mut g = self.inner.lock().unwrap();
            let origin = opts.origin.or_else(|| Some(g.node_id.clone()));
            let msg = BusMessage {
                seq,
                topic: topic.clone(),
                payload,
                retain: opts.retain,
                timestamp_ms: ts,
                expires_at_ms,
                origin,
            };
            if msg.retain {
                if msg.payload.is_null() {
                    g.retained.remove(&topic);
                } else {
                    g.retained.insert(topic.clone(), msg.clone());
                }
            }
            if g.history.len() >= MAX_BROKER_HISTORY {
                g.history.pop_front();
            }
            g.history.push_back(msg.clone());
            let matching: Vec<Arc<SubQueue>> = g
                .subscribers
                .iter()
                .filter(|s| {
                    if let (Some(ign), Some(orig)) = (&s.ignore_origin, &msg.origin) {
                        if ign == orig {
                            return false;
                        }
                    }
                    s.pattern.matches(&topic)
                })
                .cloned()
                .collect();
            (msg, matching)
        };
        self.history_cv.notify_all();
        for s in matching {
            s.push(msg.clone());
        }
        Ok(msg)
    }

    /// Convenience helper to publish any `Serialize` payload.
    pub fn publish_typed<T: Serialize>(
        &self,
        topic: impl Into<String>,
        payload: &T,
        opts: PublishOptions,
    ) -> Result<BusMessage, String> {
        let val = serde_json::to_value(payload).map_err(|e| e.to_string())?;
        self.publish(topic, val, opts).map_err(|e| e.to_string())
    }

    /// Subscribes to a wildcard pattern (`+`, `#`).
    pub fn subscribe(
        &self,
        pattern: impl Into<String>,
        opts: SubscribeOptions,
    ) -> Result<Subscription, TopicError> {
        let pat = TopicPattern::parse(pattern)?;
        let cap = if opts.capacity == 0 { 512 } else { opts.capacity };
        let now = now_unix_ms();

        let sub = {
            let mut g = self.inner.lock().unwrap();
            g.retained.retain(|_, m| !m.is_expired(now));
            let id = g.next_sub_id;
            g.next_sub_id += 1;

            let mut initial = VecDeque::new();
            if opts.replay_retained {
                for (topic, msg) in &g.retained {
                    if !pat.matches(topic) {
                        continue;
                    }
                    if let (Some(ign), Some(orig)) = (&opts.ignore_origin, &msg.origin) {
                        if ign == orig {
                            continue;
                        }
                    }
                    if initial.len() >= cap {
                        initial.pop_front();
                    }
                    initial.push_back(msg.clone());
                }
            }

            let sq = Arc::new(SubQueue {
                id,
                pattern: pat,
                ignore_origin: opts.ignore_origin,
                capacity: cap,
                queue: Mutex::new(initial),
                cv: Condvar::new(),
                dropped: AtomicU64::new(0),
            });
            g.subscribers.push(sq.clone());
            sq
        };

        Ok(Subscription {
            broker: self.clone(),
            sub,
        })
    }

    fn unsubscribe(&self, sub_id: u64) {
        let mut g = self.inner.lock().unwrap();
        g.subscribers.retain(|s| s.id != sub_id);
    }

    /// Returns a single retained message for an exact topic if present and unexpired.
    pub fn get_retained(&self, topic: &str) -> Option<BusMessage> {
        let now = now_unix_ms();
        let mut g = self.inner.lock().unwrap();
        if let Some(m) = g.retained.get(topic) {
            if m.is_expired(now) {
                g.retained.remove(topic);
                return None;
            }
            return Some(m.clone());
        }
        None
    }

    /// Queries all unexpired retained messages matching an MQTT-style wildcard pattern.
    pub fn query_retained(&self, pattern: &str) -> Result<Vec<BusMessage>, TopicError> {
        validate_pattern(pattern)?;
        let pat = TopicPattern::parse(pattern)?;
        let now = now_unix_ms();
        let mut g = self.inner.lock().unwrap();
        g.retained.retain(|_, m| !m.is_expired(now));
        Ok(g.retained
            .iter()
            .filter(|(t, _)| pat.matches(t))
            .map(|(_, m)| m.clone())
            .collect())
    }

    /// Returns matching messages with `seq > since_seq`, waiting up to `wait` if none are available yet.
    pub fn poll_events_since(
        &self,
        pattern: &str,
        since_seq: u64,
        wait: std::time::Duration,
    ) -> Result<(Vec<BusMessage>, u64), TopicError> {
        let pat = TopicPattern::parse(pattern)?;
        let deadline = std::time::Instant::now() + wait;
        let mut g = self.inner.lock().unwrap();
        loop {
            let matched: Vec<BusMessage> = g
                .history
                .iter()
                .filter(|m| m.seq > since_seq && pat.matches(&m.topic))
                .cloned()
                .collect();
            let cur_seq = self.latest_seq();
            if !matched.is_empty() || wait.is_zero() {
                return Ok((matched, cur_seq));
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok((Vec::new(), cur_seq));
            }
            let rem = deadline.saturating_duration_since(now);
            let (ng, _) = self.history_cv.wait_timeout(g, rem).unwrap();
            g = ng;
        }
    }

    /// Exports all unexpired retained messages (e.g. for disk persistence or initial bridge sync).
    pub fn snapshot_retained(&self) -> Vec<BusMessage> {
        let now = now_unix_ms();
        let mut g = self.inner.lock().unwrap();
        g.retained.retain(|_, m| !m.is_expired(now));
        g.retained.values().cloned().collect()
    }

    /// Imports retained messages from disk or a remote peer without overwriting fresher timestamps.
    pub fn restore_retained(&self, messages: Vec<BusMessage>) -> usize {
        let now = now_unix_ms();
        let mut restored = 0;
        for msg in messages {
            if msg.is_expired(now) || validate_topic(&msg.topic).is_err() {
                continue;
            }
            let opts = PublishOptions {
                retain: true,
                ttl_ms: msg.expires_at_ms.map(|e| e.saturating_sub(msg.timestamp_ms)),
                origin: msg.origin.clone(),
                timestamp_ms: Some(msg.timestamp_ms),
            };
            if self.publish(msg.topic, msg.payload, opts).is_ok() {
                restored += 1;
            }
        }
        restored
    }
}
