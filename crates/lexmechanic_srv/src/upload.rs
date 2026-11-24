//! Streaming a request body to a file (#883).
//!
//! Taking `body: Bytes` in a handler makes axum buffer the whole request
//! before the handler runs, so a 200MB book is 200MB resident before a
//! single byte reaches disk -- and if several are dropped at once, peak
//! memory is roughly their sum. Every upload handler here writes its
//! body straight to a temp file and never looks at the bytes again, so
//! none of them needed it in memory in the first place.
//!
//! `DefaultBodyLimit` still applies to a streamed body, so the size cap
//! is not lost by taking `Body` instead of `Bytes`.

use axum::body::Body;
use futures_util::StreamExt;
use std::path::Path;
use tokio::io::AsyncWriteExt;

use crate::errors::ServerError;

/// Writes `body` to `path` as it arrives, returning how many bytes landed.
///
/// A failed write removes the partial file. Leaving it behind would hand
/// the caller a truncated book that looks like a real one -- and in a
/// library folder, the next scan would index it as a book of its own.
pub async fn stream_to_file(body: Body, path: &Path) -> Result<u64, ServerError> {
    let mut file = tokio::fs::File::create(path).await.map_err(|e| ServerError::InternalServerError(format!("creating {}: {e}", path.display())))?;

    let mut stream = body.into_data_stream();
    let mut written: u64 = 0;

    while let Some(chunk) = stream.next().await {
        // A client that disconnects mid-upload lands here. It is the
        // caller's problem, not a server fault, so it is a 400 -- but
        // either way the half-written file must not survive.
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(path).await;
                return Err(ServerError::BadRequest(format!("the upload ended early: {e}")));
            }
        };
        if let Err(e) = file.write_all(&chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(path).await;
            return Err(ServerError::InternalServerError(format!("writing {}: {e}", path.display())));
        }
        written += chunk.len() as u64;
    }

    // Flushed explicitly rather than left to the drop: a buffered tail
    // lost at close would truncate the file silently, and the whole
    // point of this path is that what lands on disk is the whole book.
    file.flush().await.map_err(|e| ServerError::InternalServerError(format!("flushing {}: {e}", path.display())))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_a_body_to_the_file_and_counts_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");

        let written = stream_to_file(Body::from(b"epub bytes".to_vec()), &path).await.unwrap();

        assert_eq!(written, 10);
        assert_eq!(std::fs::read(&path).unwrap(), b"epub bytes");
    }

    /// The body arrives in pieces, which is the whole point -- the file
    /// must be the concatenation, not the last chunk.
    #[tokio::test]
    async fn reassembles_a_body_that_arrives_in_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![Ok(b"first ".to_vec()), Ok(b"second ".to_vec()), Ok(b"third".to_vec())];
        let body = Body::from_stream(futures_util::stream::iter(chunks));

        let written = stream_to_file(body, &path).await.unwrap();

        assert_eq!(written, 18);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first second third");
    }

    #[tokio::test]
    async fn an_empty_body_makes_an_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nothing.epub");

        assert_eq!(stream_to_file(Body::empty(), &path).await.unwrap(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"");
    }

    /// A client that vanishes mid-upload must not leave a truncated book
    /// behind: in a library folder the next scan would index it.
    #[tokio::test]
    async fn a_body_that_fails_midway_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("half.epub");
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![Ok(b"the first half".to_vec()), Err(std::io::Error::other("connection reset"))];
        let body = Body::from_stream(futures_util::stream::iter(chunks));

        let err = stream_to_file(body, &path).await.unwrap_err();

        assert!(matches!(err, ServerError::BadRequest(_)), "a client disconnect is the client's problem: {err:?}");
        assert!(!path.exists(), "the partial file must be cleaned up, not left looking like a real book");
    }
}
