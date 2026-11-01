//! Prometheus metrics HTTP handler using Axum.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use prometheus::{Encoder, TextEncoder};

use crate::metrics::registry::REGISTRY;

/// HTTP handler that serves Prometheus metrics in text exposition format.
pub async fn metrics_handler() -> Response {
    tokio::task::spawn_blocking(encode_metrics)
        .await
        .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
}

fn encode_metrics() -> Response {
    let metric_families = REGISTRY.gather();
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();

    match encoder.encode(&metric_families, &mut buffer) {
        Ok(()) => (
            StatusCode::OK,
            [("Content-Type", "text/plain; version=0.0.4; charset=utf-8")],
            buffer,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to encode metrics: {e}"),
        )
            .into_response(),
    }
}

/// Health check endpoint.
pub async fn health_handler() -> Response {
    (StatusCode::OK, "OK").into_response()
}
