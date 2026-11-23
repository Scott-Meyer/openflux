//! Subscription-driven MQTT-style Broker-to-Broker Mirroring (`CH_BUS`).
//!
//! Connects a host `Broker` and a keyboard `Broker` as symmetric peers:
//! - Each side advertises the exact topic patterns its local modules/clients care about via
//!   `PeerBusFrame::Subscribe { patterns }`.
//! - The remote peer immediately replays matching retained state ONCE and streams every future
//!   matching publish in `< 1 ms` with zero polling.
//! - Every frame carries `source_node`, so loop-prevention is a single invariant (`source != remote`).
//! - On disconnect, each peer runs Erlang/MQTT LWT cleanup (`purge_origin("peer:<remote>")`).

use crate::broker::{Broker, BusMessage, PublishOptions, SubscribeOptions, Subscription};
use crate::mux::{MuxChannel, MuxFrame};
use crate::topic::TopicPattern;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerBusFrame {
    /// Advertises the full set of topic patterns this peer wants mirrored from the remote broker.
    /// Carries a per-process `session_id` so if either peer restarts independently in ANY order,
    /// the surviving side automatically re-advertises its own subscriptions and replays retained state.
    Subscribe {
        node: String,
        #[serde(default)]
        session_id: u64,
        patterns: Vec<String>,
    },
    /// Mirrors a single published message (one-off event or retained state) to the remote broker.
    Publish {
        source_node: String,
        topic: String,
        payload: Value,
        #[serde(default)]
        retain: bool,
        #[serde(default)]
        ttl_ms: Option<u64>,
        #[serde(default)]
        origin: Option<String>,
    },
    /// Propagates Erlang/MQTT Last-Will origin cleanup across the link.
    PurgeOrigin {
        source_node: String,
        origin: String,
    },
    Ping {
        ts_ms: u64,
    },
    Pong {
        ts_ms: u64,
    },
}

impl PeerBusFrame {
    pub fn to_mux_frame(&self) -> MuxFrame {
        let payload = serde_json::to_vec(self).unwrap_or_default();
        MuxFrame::new(MuxChannel::Bus, 0, 0, payload)
    }

    pub fn from_mux_frame(frame: &MuxFrame) -> Option<Self> {
        if frame.channel != MuxChannel::Bus {
            return None;
        }
        serde_json::from_slice(&frame.payload).ok()
    }
}

/// Stateful peer bridge managing subscription-driven mirroring between two `Broker` instances.
pub struct BrokerMirror {
    broker: Broker,
    local_node: String,
    remote_node: String,
    peer_origin_prefix: String,
    session_id: u64,
    last_remote_session_id: Option<u64>,
    local_sub: Subscription,
    remote_patterns: Vec<TopicPattern>,
    last_advertised_patterns: Vec<String>,
}

impl BrokerMirror {
    pub fn new(broker: Broker, remote_node: impl Into<String>) -> Self {
        static NEXT_SID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let local_node = broker.node_id().to_string();
        let remote_node = remote_node.into();
        let peer_origin_prefix = format!("peer:{remote_node}");
        let sid_seq = NEXT_SID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let session_id = (crate::broker::now_unix_ms() << 16) ^ sid_seq ^ (std::process::id() as u64);
        let local_sub = broker
            .subscribe(
                "#",
                SubscribeOptions {
                    replay_retained: false,
                    ignore_origin: Some(peer_origin_prefix.clone()),
                    capacity: 2048,
                },
            )
            .expect("wildcard # pattern is always valid");

        Self {
            broker,
            local_node,
            remote_node,
            peer_origin_prefix,
            session_id,
            last_remote_session_id: None,
            local_sub,
            remote_patterns: Vec::new(),
            last_advertised_patterns: Vec::new(),
        }
    }

    pub fn subscription(&self) -> &Subscription {
        &self.local_sub
    }

    /// Computes and returns a `Subscribe` frame whenever our local desired patterns change
    /// (or always when `force == true`, e.g. on initial link connect).
    pub fn build_subscribe_if_changed(
        &mut self,
        extra_patterns: &[String],
        force: bool,
    ) -> Option<PeerBusFrame> {
        let mut set: BTreeSet<String> = self
            .broker
            .active_subscription_patterns_excluding(self.local_sub.id())
            .into_iter()
            .collect();
        for p in extra_patterns {
            if !p.trim().is_empty() {
                set.insert(p.clone());
            }
        }
        let next: Vec<String> = set.into_iter().collect();
        if !force && next == self.last_advertised_patterns {
            return None;
        }
        self.last_advertised_patterns = next.clone();
        Some(PeerBusFrame::Subscribe {
            node: self.local_node.clone(),
            session_id: self.session_id,
            patterns: next,
        })
    }

    /// Handles an incoming `PeerBusFrame` from the remote peer, returning any immediate
    /// response frames (such as our own `Subscribe` if the peer has a new `session_id`,
    /// retained snapshots for newly subscribed patterns, or `Pong`).
    pub fn on_remote_frame(&mut self, frame: PeerBusFrame) -> Vec<PeerBusFrame> {
        match frame {
            PeerBusFrame::Subscribe {
                session_id,
                patterns,
                ..
            } => {
                self.remote_patterns = patterns
                    .iter()
                    .filter_map(|p| TopicPattern::parse(p).ok())
                    .collect();

                let mut replies = Vec::new();
                // Symmetric session convergence: if the remote peer is new or restarted
                // (new `session_id`), automatically send our own `Subscribe` back so BOTH
                // directions converge in 1 RTT regardless of boot/restart order!
                if session_id != 0 && self.last_remote_session_id != Some(session_id) {
                    self.last_remote_session_id = Some(session_id);
                    if !self.last_advertised_patterns.is_empty() {
                        replies.push(PeerBusFrame::Subscribe {
                            node: self.local_node.clone(),
                            session_id: self.session_id,
                            patterns: self.last_advertised_patterns.clone(),
                        });
                    }
                }

                // Replay all unexpired retained messages matching the peer's requested patterns!
                let mut seen_topics = BTreeSet::new();
                for pat in &patterns {
                    if let Ok(msgs) = self.broker.query_retained(pat) {
                        for msg in msgs {
                            if seen_topics.insert(msg.topic.clone()) {
                                if let Some(pf) = self.maybe_outgoing_frame(&msg) {
                                    replies.push(pf);
                                }
                            }
                        }
                    }
                }
                replies
            }
            PeerBusFrame::Publish {
                source_node,
                topic,
                payload,
                retain,
                ttl_ms,
                ..
            } => {
                // Loop immunity invariant: never apply frames that originated from our own node.
                if source_node == self.local_node {
                    return Vec::new();
                }
                let opts = PublishOptions {
                    retain,
                    ttl_ms,
                    origin: Some(self.peer_origin_prefix.clone()),
                    timestamp_ms: None,
                };
                let _ = self.broker.publish(topic, payload, opts);
                Vec::new()
            }
            PeerBusFrame::PurgeOrigin {
                source_node,
                origin,
            } => {
                if source_node != self.local_node {
                    let _ = self
                        .broker
                        .purge_origin(&format!("{}:{origin}", self.peer_origin_prefix));
                }
                Vec::new()
            }
            PeerBusFrame::Ping { ts_ms } => vec![PeerBusFrame::Pong { ts_ms }],
            PeerBusFrame::Pong { .. } => Vec::new(),
        }
    }

    /// Converts a local `BusMessage` into an outgoing `PeerBusFrame` if and only if:
    /// 1. It did NOT come from the remote peer (loop-free), AND
    /// 2. At least one of the remote peer's subscribed patterns matches `msg.topic`.
    pub fn maybe_outgoing_frame(&self, msg: &BusMessage) -> Option<PeerBusFrame> {
        if let Some(ref orig) = msg.origin {
            if orig == &self.peer_origin_prefix || orig.starts_with(&format!("{}:", self.peer_origin_prefix)) {
                return None;
            }
        }
        if !self.remote_patterns.iter().any(|p| p.matches(&msg.topic)) {
            return None;
        }
        Some(PeerBusFrame::Publish {
            source_node: self.local_node.clone(),
            topic: msg.topic.clone(),
            payload: msg.payload.clone(),
            retain: msg.retain,
            ttl_ms: msg
                .expires_at_ms
                .map(|exp| exp.saturating_sub(msg.timestamp_ms)),
            origin: msg.origin.clone(),
        })
    }

    /// Drains all newly published local messages that match the remote peer's subscriptions.
    pub fn drain_outgoing(&self) -> Vec<PeerBusFrame> {
        self.local_sub
            .drain()
            .into_iter()
            .filter_map(|m| self.maybe_outgoing_frame(&m))
            .collect()
    }

    /// Called when the USB link disconnects: cleans up peer-owned retained topics (LWT) and resets state.
    pub fn on_disconnect(&mut self) {
        self.remote_patterns.clear();
        self.last_advertised_patterns.clear();
        self.last_remote_session_id = None;
        let _ = self.broker.purge_origin(&self.peer_origin_prefix);
        let _ = self.broker.publish_typed(
            crate::schema::topics::HOST_CONNECTION,
            &crate::schema::HostConnectionState {
                connected: false,
                host_asleep: false,
                last_seen_ms: crate::broker::now_unix_ms(),
                host_platform: None,
            },
            PublishOptions::retained(),
        );
    }

    pub fn remote_node(&self) -> &str {
        &self.remote_node
    }
}
