use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;


#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Forbidden(String),
    Conflict(String),
    RateLimited,
    ExternalApi(String),
    Internal(String),
}


#[derive(Serialize, ToSchema)]
pub(crate) struct ErrorResponse {
    pub error: String,
}


impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "Rate limited".into()),
            Self::ExternalApi(ref msg) => {
                tracing::error!("External API error: {msg}");
                (StatusCode::BAD_GATEWAY, msg.clone())
            }
            Self::Internal(ref msg) => {
                tracing::error!("Internal error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}


impl From<clients::ClientError> for ApiError {
    fn from(err: clients::ClientError) -> Self {
        match err {
            clients::ClientError::PlayerNotFound(p) => Self::NotFound(format!("Player not found: {p}")),
            clients::ClientError::RateLimited => Self::RateLimited,
            clients::ClientError::HypixelApi(msg) => Self::ExternalApi(msg),
            clients::ClientError::InvalidUuid(u) => Self::BadRequest(format!("Invalid UUID: {u}")),
            _ => Self::Internal(err.to_string()),
        }
    }
}


impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        Self::Internal(format!("Database error: {err}"))
    }
}
