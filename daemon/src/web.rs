use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post, put},
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use tracing::error;

use crate::config::{DualieConfig, VirtualAction};
use crate::platform;
use crate::serialize;
use crate::AppState;

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        // Config
        .route("/config",              get(get_config).put(put_config))
        .route("/config/download",     get(download_config))
        .route("/config/upload",       post(upload_config))
        .route("/config/binary",       get(binary_config))
        // Device (thin wrappers – heavy lifting done in-browser via WebHID)
        .route("/device/status",       get(device_status))
        .route("/device/output",       put(put_active_output))
        // Platform discovery
        .route("/platform/info",       get(platform_info))
        .route("/platform/apps",       get(platform_apps))
        // Keycode table
        .route("/keycodes",            get(keycodes))
        // Virtual action CRUD
        .route("/outputs/:idx/actions",           get(get_actions))
        .route("/outputs/:idx/actions/:slot",     put(put_action))
        .with_state(Arc::clone(&state));

    // In dev mode the SPA is served by Vite; in prod embed the built dist/
    let app = Router::new().nest("/api/v1", api);

    if state.dev_mode {
        app
    } else {
        // Serve the embedded SPA bundle
        app.fallback(serve_spa)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Unified JSON error response
#[derive(Serialize)]
struct ApiError { error: String }

fn err(code: StatusCode, msg: impl ToString) -> Response {
    (code, Json(ApiError { error: msg.to_string() })).into_response()
}

// ── Config endpoints ──────────────────────────────────────────────────────────

async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.config.read().await.clone())
}

async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(new_cfg): Json<DualieConfig>,
) -> impl IntoResponse {
    let mut cfg = state.config.write().await;
    *cfg = new_cfg;
    if let Err(e) = cfg.save() {
        error!("Failed to save config: {e}");
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn download_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.config.read().await.to_cbor() {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/cbor"),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"dualie-config.cbor\""),
            ],
            bytes,
        ).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn upload_config(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    match DualieConfig::from_cbor(&body) {
        Ok(new_cfg) => {
            let mut cfg = state.config.write().await;
            *cfg = new_cfg;
            if let Err(e) = cfg.save() {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, format!("Invalid config: {e}")),
    }
}

/// Return the current config serialised as the raw firmware `config_t` blob.
/// The browser calls this, then pushes the bytes to the device via WebHID.
async fn binary_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg   = state.config.read().await;
    let bytes = serialize::config_to_bytes(&cfg);
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "inline"),
        ],
        bytes,
    )
        .into_response()
}

// ── Virtual action endpoints ──────────────────────────────────────────────────

async fn get_actions(
    State(state): State<Arc<AppState>>,
    Path(idx): Path<usize>,
) -> impl IntoResponse {
    if idx >= 2 {
        return err(StatusCode::NOT_FOUND, "output index must be 0 or 1");
    }
    let cfg = state.config.read().await;
    Json(cfg.outputs[idx].virtual_actions.clone()).into_response()
}

async fn put_action(
    State(state): State<Arc<AppState>>,
    Path((idx, slot)): Path<(usize, usize)>,
    Json(action): Json<VirtualAction>,
) -> impl IntoResponse {
    if idx >= 2 {
        return err(StatusCode::NOT_FOUND, "output index must be 0 or 1");
    }
    if slot >= crate::config::DUALIE_VKEY_COUNT {
        return err(StatusCode::BAD_REQUEST, "slot must be 0-31");
    }
    let mut cfg = state.config.write().await;
    cfg.outputs[idx].virtual_actions[slot] = action;
    if let Err(e) = cfg.save() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    StatusCode::NO_CONTENT.into_response()
}

// ── Device status ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DeviceStatus {
    /// The daemon itself doesn't hold the HID connection – the browser does
    /// via WebHID.  This endpoint just reports daemon health.
    daemon_ok: bool,
}

async fn device_status() -> impl IntoResponse {
    Json(DeviceStatus { daemon_ok: true })
}

#[derive(serde::Deserialize)]
struct OutputBody { index: usize }

async fn put_active_output(
    State(state): State<Arc<AppState>>,
    Json(body): Json<OutputBody>,
) -> impl IntoResponse {
    if body.index >= 2 {
        return err(StatusCode::BAD_REQUEST, "output index must be 0 or 1");
    }
    state.device.set_active_output(body.index);
    StatusCode::NO_CONTENT.into_response()
}

// ── Platform discovery ────────────────────────────────────────────────────────

async fn platform_info() -> impl IntoResponse {
    Json(platform::system_info())
}

async fn platform_apps() -> impl IntoResponse {
    match platform::list_apps() {
        Ok(apps) => Json(apps).into_response(),
        Err(e)   => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

// ── Keycode table ─────────────────────────────────────────────────────────────

async fn keycodes() -> impl IntoResponse {
    // Serve the shared/keycodes.json embedded at compile time
    let json = include_str!("../../shared/keycodes.json");
    ([(header::CONTENT_TYPE, "application/json")], json).into_response()
}

// ── SPA fallback ──────────────────────────────────────────────────────────────

async fn serve_spa() -> impl IntoResponse {
    // The SPA index.html is embedded at compile time from the web build output.
    // In dev mode this branch is never reached.
    let html = include_str!("web/static/index.html");
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}
