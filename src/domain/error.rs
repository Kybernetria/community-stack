use std::fmt;

/// Stable error categories used at the local API boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorCode {
    Unauthenticated,
    Forbidden,
    Conflict,
    RevisionConflict,
    IdempotencyConflict,
    MethodNotFound,
    InvalidRequest,
    InvalidJson,
}

impl ApiErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::Forbidden => "FORBIDDEN",
            Self::Conflict => "CONFLICT",
            Self::RevisionConflict => "REVISION_CONFLICT",
            Self::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            Self::MethodNotFound => "METHOD_NOT_FOUND",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::InvalidJson => "INVALID_JSON",
        }
    }
}

/// A typed, client-safe error marker.
///
/// The marker is attached to an internal error chain. The API serializes only
/// this value; the underlying anyhow cause remains available to the process.
#[derive(Debug)]
pub struct ApiError {
    code: ApiErrorCode,
    message: &'static str,
    retryable: bool,
}

impl ApiError {
    pub const fn new(code: ApiErrorCode, message: &'static str, retryable: bool) -> Self {
        Self {
            code,
            message,
            retryable,
        }
    }

    pub const fn code(&self) -> ApiErrorCode {
        self.code
    }

    pub const fn message(&self) -> &'static str {
        self.message
    }

    pub const fn retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for ApiError {}
