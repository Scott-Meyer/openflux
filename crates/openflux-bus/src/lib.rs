//! OpenFlux v2 Hierarchical Pub/Sub Bus, Physical Keyboard Geometry, and Shared Semantic Contracts.

pub mod broker;
pub mod geometry;
pub mod schema;
pub mod topic;
pub mod wire;

pub use broker::{now_unix_ms, Broker, BusMessage, PublishOptions, SubscribeOptions, Subscription};
pub use geometry::{
    FrameLayout, KeyGeometry, KeySdfSample, KeyboardGeometry, RectPx, SlotGeometry,
    DISPLAY_HEIGHT_MM, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_MM, DISPLAY_WIDTH_PX, PX_PER_MM,
};
pub use schema::{
    topics, AnalogKeyFrame, AnalogKeySample, HostConnectionState, KeyEvent, KeyEventKind,
    KeyboardState, MediaNowPlaying, SurfaceImpulse, SystemMetrics, WeatherHourly, WeatherState,
    MAX_TRAVEL_UM,
};
pub use topic::{topic_matches, validate_pattern, validate_topic, TopicError, TopicPattern};
pub use wire::{apply_remote_frame, WireFrame};
