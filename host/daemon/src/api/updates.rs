//! Routes for update discovery, keyboard updates, conversion and jobs (first-party only).

use crate::api::state::ApiState;
use crate::updates::{Refused, UpdateService};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use std::sync::Arc;

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn refused(r: Refused) -> Response {
    error(
        StatusCode::from_u16(r.status).unwrap_or(StatusCode::CONFLICT),
        r.code,
        &r.message,
    )
}

fn service(state: &ApiState) -> Result<Arc<UpdateService>, Box<Response>> {
    state.updates().ok_or_else(|| {
        Box::new(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Updates aren't available in this daemon",
        ))
    })
}

async fn get_updates(State(state): State<Arc<ApiState>>) -> Response {
    match service(&state) {
        Ok(u) => (StatusCode::OK, Json(u.status_body())).into_response(),
        Err(r) => *r,
    }
}

async fn post_check(State(state): State<Arc<ApiState>>) -> Response {
    match service(&state) {
        Ok(u) => {
            u.request_check();
            (StatusCode::OK, Json(u.status_body())).into_response()
        }
        Err(r) => *r,
    }
}

async fn put_settings(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let u = match service(&state) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let channel = match body.get("channel") {
        None => None,
        Some(v) => match v.as_str().map(str::parse::<openflux_update::Channel>) {
            Some(Ok(c)) => Some(c),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "badRequest",
                    "channel must be stable or beta",
                )
            }
        },
    };
    let auto_check = match body.get("autoCheck") {
        None => None,
        Some(v) => match v.as_bool() {
            Some(b) => Some(b),
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "badRequest",
                    "autoCheck must be true or false",
                )
            }
        },
    };
    if let Err(e) = u.put_settings(channel, auto_check) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            &format!("The settings couldn't be saved: {e}"),
        );
    }
    (StatusCode::OK, Json(u.status_body())).into_response()
}

async fn spawn_job(
    state: Arc<ApiState>,
    f: impl FnOnce(Arc<UpdateService>) -> Result<String, Refused> + Send + 'static,
) -> Response {
    let u = match service(&state) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    // Admission reads the keyboard, so it runs off the async runtime.
    match tokio::task::spawn_blocking(move || f(u)).await {
        Ok(Ok(id)) => {
            (StatusCode::ACCEPTED, Json(serde_json::json!({"jobId": id}))).into_response()
        }
        Ok(Err(r)) => refused(r),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            &e.to_string(),
        ),
    }
}

/// Parses an optional JSON object request body. Empty bodies yield `Ok(None)`;
/// malformed JSON or non-object values return `400 badRequest` instead of being ignored.
fn parse_optional_json_object(body: &Bytes) -> Result<Option<serde_json::Value>, Box<Response>> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(None);
    }
    let val: serde_json::Value = serde_json::from_slice(body).map_err(|e| {
        Box::new(error(
            StatusCode::BAD_REQUEST,
            "badRequest",
            &format!("Invalid JSON body: {e}"),
        ))
    })?;
    if !val.is_object() {
        return Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "badRequest",
            "Request body must be a JSON object",
        )));
    }
    Ok(Some(val))
}

fn parse_components_field(
    body: Option<&serde_json::Value>,
) -> Result<Option<Vec<String>>, Box<Response>> {
    match body.and_then(|b| b.get("components")) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(a)) => match a
            .iter()
            .map(|v| v.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()
        {
            Some(list) => Ok(Some(list)),
            None => Err(Box::new(error(
                StatusCode::BAD_REQUEST,
                "badRequest",
                "components must be a list of component names",
            ))),
        },
        Some(_) => Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "badRequest",
            "components must be a list of component names",
        ))),
    }
}

fn parse_expected_steps(
    body: Option<&serde_json::Value>,
) -> Result<Option<Vec<(String, String)>>, Box<Response>> {
    match body.and_then(|b| b.get("expectedSteps")) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(a)) => {
            let mut out = Vec::with_capacity(a.len());
            for item in a {
                let comp = item.get("component").and_then(|v| v.as_str());
                let to = item.get("to").and_then(|v| v.as_str());
                match (comp, to) {
                    (Some(c), Some(t)) => out.push((c.to_string(), t.to_string())),
                    _ => {
                        return Err(Box::new(error(
                            StatusCode::BAD_REQUEST,
                            "badRequest",
                            "expectedSteps must be a list of {component, to} objects",
                        )))
                    }
                }
            }
            Ok(Some(out))
        }
        Some(_) => Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "badRequest",
            "expectedSteps must be a list of {component, to} objects",
        ))),
    }
}

async fn post_plan(State(state): State<Arc<ApiState>>, body: Bytes) -> Response {
    let parsed = match parse_optional_json_object(&body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let components = match parse_components_field(parsed.as_ref()) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let u = match service(&state) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match u.plan_body(components) {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(r) => refused(r),
    }
}

async fn post_apply(State(state): State<Arc<ApiState>>, body: Bytes) -> Response {
    let parsed = match parse_optional_json_object(&body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let components = match parse_components_field(parsed.as_ref()) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let expected_steps = match parse_expected_steps(parsed.as_ref()) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    spawn_job(state, move |u| u.apply(components, expected_steps)).await
}

fn confirmed(body: Option<&serde_json::Value>) -> bool {
    body.and_then(|b| b.get("confirm"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

async fn get_conversion(State(state): State<Arc<ApiState>>) -> Response {
    let u = match service(&state) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match tokio::task::spawn_blocking(move || u.conversion_body()).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            &e.to_string(),
        ),
    }
}

async fn post_convert(State(state): State<Arc<ApiState>>, body: Bytes) -> Response {
    let parsed = match parse_optional_json_object(&body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let confirm = confirmed(parsed.as_ref());
    spawn_job(state, move |u| u.convert(confirm)).await
}

async fn post_restore(State(state): State<Arc<ApiState>>, body: Bytes) -> Response {
    let parsed = match parse_optional_json_object(&body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let confirm = confirmed(parsed.as_ref());
    spawn_job(state, move |u| u.restore(confirm)).await
}

async fn get_job(State(state): State<Arc<ApiState>>, Path(id): Path<String>) -> Response {
    let u = match service(&state) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match u.job(&id) {
        Some(j) => (StatusCode::OK, Json(j)).into_response(),
        None => error(StatusCode::NOT_FOUND, "notFound", "No such job"),
    }
}

pub fn router() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/v1/updates", get(get_updates))
        .route("/v1/updates/check", post(post_check))
        .route("/v1/updates/settings", put(put_settings))
        .route("/v1/updates/plan", post(post_plan))
        .route("/v1/updates/apply", post(post_apply))
        .route("/v1/conversion", get(get_conversion))
        .route("/v1/conversion/convert", post(post_convert))
        .route("/v1/conversion/restore", post(post_restore))
        .route("/v1/jobs/:id", get(get_job))
}
