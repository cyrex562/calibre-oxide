//! Port of `calibre.srv.errors`. Upstream's `HTTPSimpleResponse` family
//! is a set of exception types the hand-rolled event loop's exception
//! handler (`http_response.py`) catches and turns into an HTTP
//! response. `axum` has a direct built-in equivalent for exactly that
//! pattern -- any type implementing `IntoResponse` returned as a
//! handler's `Err` becomes the response -- so [`ServerError`] is a
//! single enum implementing it, rather than a small exception class
//! hierarchy.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

/// Port of the `HTTPSimpleResponse` family.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("authentication required")]
    Unauthorized,
    #[error("{0}")]
    FailedDependency(String),
    #[error("{0}")]
    PreconditionRequired(String),
    #[error("{0}")]
    UnprocessableEntity(String),
    #[error("{0}")]
    InternalServerError(String),
    /// Port of `HTTPRedirect`/`HTTPTempRedirect`. `permanent` selects
    /// between them (301 vs 307).
    #[error("redirect to {location}")]
    Redirect { location: String, permanent: bool },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ServerError {
    /// Port of `BookNotFound`.
    pub fn book_not_found(book_id: i32, library_id: &str) -> ServerError {
        ServerError::NotFound(format!("No book with id: {book_id} in library: {library_id}"))
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        match self {
            ServerError::NotFound(msg) => (StatusCode::NOT_FOUND, msg).into_response(),
            ServerError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            ServerError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg).into_response(),
            ServerError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, [("WWW-Authenticate", "Basic realm=\"calibre server\"")], "authentication required").into_response()
            }
            ServerError::FailedDependency(msg) => (StatusCode::FAILED_DEPENDENCY, msg).into_response(),
            ServerError::PreconditionRequired(msg) => (StatusCode::PRECONDITION_REQUIRED, msg).into_response(),
            ServerError::UnprocessableEntity(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response(),
            ServerError::InternalServerError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
            ServerError::Redirect { location, permanent } => {
                if permanent {
                    Redirect::permanent(&location).into_response()
                } else {
                    Redirect::temporary(&location).into_response()
                }
            }
            ServerError::Other(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_not_found_formats_the_same_message_as_upstream() {
        let err = ServerError::book_not_found(42, "my_library");
        assert_eq!(err.to_string(), "No book with id: 42 in library: my_library");
    }

    #[test]
    fn into_response_maps_variants_to_the_right_status_codes() {
        assert_eq!(ServerError::NotFound("x".into()).into_response().status(), StatusCode::NOT_FOUND);
        assert_eq!(ServerError::BadRequest("x".into()).into_response().status(), StatusCode::BAD_REQUEST);
        assert_eq!(ServerError::Unauthorized.into_response().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(ServerError::Forbidden("x".into()).into_response().status(), StatusCode::FORBIDDEN);
    }
}
