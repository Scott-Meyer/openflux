//! Wire framing for bridging two Keyglass brokers (Keyboard <-> Host USB, Host <-> WS/CLI).

use crate::broker::{Broker, BusMessage, PublishOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Single protocol frame exchanged over WebSocket, Unix domain socket, or USB bridge channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WireFrame {
    Publish {
        topic: String,
        payload: Value,
        #[serde(default)]
        retain: bool,
        #[serde(default)]
        ttl_ms: Option<u64>,
        #[serde(default)]
        origin: Option<String>,
    },
    Subscribe {
        pattern: String,
        #[serde(default = "default_true")]
        replay_retained: bool,
    },
    Unsubscribe {
        pattern: String,
    },
    Message {
        message: BusMessage,
    },
    Batch {
        frames: Vec<WireFrame>,
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    Error {
        message: String,
    },
}

fn default_true() -> bool {
    true
}

impl WireFrame {
    pub fn from_bus_message(msg: &BusMessage) -> Self {
        Self::Publish {
            topic: msg.topic.clone(),
            payload: msg.payload.clone(),
            retain: msg.retain,
            ttl_ms: msg
                .expires_at_ms
                .map(|exp| exp.saturating_sub(msg.timestamp_ms)),
            origin: msg.origin.clone(),
        }
    }

    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }

    pub fn from_json_str(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim())
    }
}

/// Applies an incoming `WireFrame` (from a remote peer) to a local `Broker`, stamping
/// `remote_origin` when the frame carries none so bridged peers never echo frames back.
pub fn apply_remote_frame(
    broker: &Broker,
    frame: WireFrame,
    remote_origin: &str,
) -> Result<usize, String> {
    match frame {
        WireFrame::Publish {
            topic,
            payload,
            retain,
            ttl_ms,
            origin,
        } => {
            let opts = PublishOptions {
                retain,
                ttl_ms,
                origin: Some(origin.unwrap_or_else(|| remote_origin.to_string())),
                timestamp_ms: None,
            };
            broker
                .publish(topic, payload, opts)
                .map_err(|e| e.to_string())?;
            Ok(1)
        }
        WireFrame::Message { message } => {
            let opts = PublishOptions {
                retain: message.retain,
                ttl_ms: message
                    .expires_at_ms
                    .map(|exp| exp.saturating_sub(message.timestamp_ms)),
                origin: Some(
                    message
                        .origin
                        .unwrap_or_else(|| remote_origin.to_string()),
                ),
                timestamp_ms: Some(message.timestamp_ms),
            };
            broker
                .publish(message.topic, message.payload, opts)
                .map_err(|e| e.to_string())?;
            Ok(1)
        }
        WireFrame::Batch { frames } => {
            let mut applied = 0;
            for f in frames {
                applied += apply_remote_frame(broker, f, remote_origin)?;
            }
            Ok(applied)
        }
        _ => Ok(0),
    }
}
