use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::types::{ErrorBody, ErrorResponse, PrettyJson};

/// API error type that converts to HTTP responses.
pub enum ApiError {
    BadRequest {
        message: String,
        param: Option<String>,
    },
    NotFound {
        message: String,
    },
    Internal {
        message: String,
    },
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_type, code, message, param) = match self {
            ApiError::BadRequest { message, param } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                Some("invalid_parameter".to_string()),
                message,
                param,
            ),
            ApiError::NotFound { message } => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                Some("resource_missing".to_string()),
                message,
                None,
            ),
            ApiError::Internal { message } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                None,
                message,
                None,
            ),
        };

        let body = ErrorResponse {
            error: ErrorBody {
                error_type: error_type.to_string(),
                code,
                message,
                param,
            },
        };

        (status, PrettyJson(body)).into_response()
    }
}
