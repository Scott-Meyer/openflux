//! Routes for the keyboard's press-refinement data (first-party only).
//!
//! - `GET /v1/keyboard/press-refinement`: `{"state", "crc32"}`. `state` is `absent`, `installed`,
//!   `invalid`, `storageUnavailable`, or `unknown` when the firmware doesn't answer (then with a
//!   `code` and `message`); `crc32` is the installed payload's CRC-32, otherwise `null`. An
//!   installed state also has `active`: whether the firmware applies the data or only stores it.
//! - `POST /v1/keyboard/press-refinement/install`: obtains the data, installs it and confirms it.
//!   Without a body (or with `{}`) the data is the kept copy, or a download from the sources the
//!   verified index records. With `{"path": "<absolute path>"}` it comes from a file on this
//!   computer that the user chose: the maker's software package or the bare UF2 firmware file.
//!   Either way it is verified against the index and kept. Answers
//!   `{"state": "installed", "crc32", "active", "alreadyInstalled", "fromCache"}`.
//!
//! Both are refused with 409 while a job has the keyboard or the keyboard can't be written (the
//! codes of [`crate::updates::WriteRefusal`]). Errors are `{"error": {"code", "message"}}`.

use crate::api::state::ApiState;
use crate::conversion::press_refinement::{self, Error};
use crate::updates::Area;
use crate::usb::press_refinement::{State as StoredState, Status};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::sync::Arc;

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn refused_write(state: &ApiState) -> Option<Response> {
    state
        .check_keyboard_write(&[Area::FirmwareUsb])
        .err()
        .map(|r| error(StatusCode::CONFLICT, r.code, &r.message))
}

fn install_error(e: &Error) -> Response {
    let status = match e {
        Error::NotAvailable(_) => StatusCode::NOT_IMPLEMENTED,
        Error::File(_) => StatusCode::BAD_REQUEST,
        Error::Offline(_) => StatusCode::SERVICE_UNAVAILABLE,
        Error::Unavailable(_) | Error::HashMismatch(_) => StatusCode::BAD_GATEWAY,
        Error::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        Error::KeyboardDisconnected(_) | Error::StorageUnavailable(_) => StatusCode::CONFLICT,
        Error::KeyboardRefused(_) | Error::VerifyFailed(_) => StatusCode::BAD_GATEWAY,
    };
    error(status, e.code(), e.message())
}

fn status_body(s: &Status) -> serde_json::Value {
    let installed = s.state.is_installed();
    let mut body = serde_json::json!({
        "state": if installed { StoredState::Installed } else { s.state },
        "crc32": installed.then_some(s.payload_crc32),
    });
    if installed {
        body["active"] = serde_json::json!(s.state == StoredState::Installed);
    }
    body
}

async fn get_status(State(state): State<Arc<ApiState>>) -> Response {
    if let Some(r) = refused_write(&state) {
        return r;
    }
    let device = state.device.clone();
    match tokio::task::spawn_blocking(move || press_refinement::status(&*device)).await {
        Ok(Ok(s)) => (StatusCode::OK, Json(status_body(&s))).into_response(),
        Ok(Err(e)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "state": "unknown",
                "crc32": null,
                "code": e.code(),
                "message": e.message(),
            })),
        )
            .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            &e.to_string(),
        ),
    }
}

async fn post_install(
    State(state): State<Arc<ApiState>>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let path = match body.as_ref().and_then(|b| b.get("path")) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(p)) if std::path::Path::new(p).is_absolute() => {
            Some(std::path::PathBuf::from(p))
        }
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "badRequest",
                "path must be an absolute file path",
            )
        }
    };
    let Some(svc) = state.updates() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Press-refinement data can't be installed by this daemon",
        );
    };
    if let Some(r) = refused_write(&state) {
        return r;
    }
    let hold = match svc.hold_keyboard("installing press-refinement data") {
        Ok(h) => h,
        Err(r) => {
            return error(
                StatusCode::from_u16(r.status).unwrap_or(StatusCode::CONFLICT),
                r.code,
                &r.message,
            )
        }
    };
    let device = state.device.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _hold = hold;
        let source = match &path {
            Some(p) => press_refinement::Source::File(p),
            None => press_refinement::Source::Published,
        };
        press_refinement::obtain(&svc, source)
            .and_then(|obtained| press_refinement::install_obtained(&*device, obtained))
    })
    .await;
    match result {
        Ok(Ok(done)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "state": StoredState::Installed,
                "crc32": done.crc32,
                "active": done.active,
                "alreadyInstalled": done.already_installed,
                "fromCache": done.from_cache,
            })),
        )
            .into_response(),
        Ok(Err(e)) => install_error(&e),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            &e.to_string(),
        ),
    }
}

pub fn router() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/v1/keyboard/press-refinement", get(get_status))
        .route("/v1/keyboard/press-refinement/install", post(post_install))
}
