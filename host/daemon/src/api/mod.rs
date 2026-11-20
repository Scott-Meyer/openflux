pub mod assets;
pub mod keyboard_edits;
pub mod lifecycle_client;
pub mod press_refinement;
pub mod routes;
pub mod shortcuts;
pub mod state;
pub mod third_party;
pub mod tls;
pub mod updates;
pub mod worker;

use axum::Router;
use state::ApiState;
use std::sync::Arc;

/// The daemon's complete local API: OpenFlux's own routes and the third-party app API.
pub fn build_router(state: Arc<ApiState>) -> Router {
    routes::router(state.clone()).merge(third_party::router(state))
}
