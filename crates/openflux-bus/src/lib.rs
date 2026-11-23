//! OpenFlux v2 Hierarchical Pub/Sub Bus, Physical Keyboard Geometry, and Shared Semantic Contracts.

pub mod broker;
pub mod geometry;
pub mod mirror;
pub mod mux;
pub mod schema;
pub mod topic;
pub mod wire;

pub use broker::{now_unix_ms, Broker, BusMessage, PublishOptions, SubscribeOptions, Subscription};
pub use geometry::{
    FrameLayout, KeyGeometry, KeySdfSample, KeyboardGeometry, RectPx, SlotGeometry,
    DISPLAY_HEIGHT_MM, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_MM, DISPLAY_WIDTH_PX, PX_PER_MM,
};
pub use mirror::{BrokerMirror, PeerBusFrame};
pub use mux::{
    decode_media_payload, encode_media_payload, BulkRpcRequest, BulkRpcResponse, MediaCodec,
    MuxChannel, MuxDecoder, MuxFrame, FLAG_KEYFRAME, FLAG_STREAM_CLOSE, MUX_HEADER_LEN,
    MUX_MAGIC, V2_UPGRADE_ACK, V2_UPGRADE_MAGIC,
};
pub use schema::{
    topics, AnalogKeyFrame, AnalogKeySample, FluxActionEvent, HostConnectionState, KeyEvent,
    KeyEventKind, KeyboardState, MediaNowPlaying, ResolvedKeyEntry, ResolvedKeysState,
    SurfaceImpulse, SystemMetrics, WeatherHourly, WeatherState, MAX_TRAVEL_UM,
};
pub use topic::{topic_matches, validate_pattern, validate_topic, TopicError, TopicPattern};
pub use wire::{apply_remote_frame, WireFrame};
