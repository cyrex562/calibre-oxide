//! Port of the still-relevant parts of `calibre.srv.utils`. Most of
//! that file (`MultiDict`, `create_sock_pair`, `start_cork`/`stop_cork`,
//! `RotatingStream`/`RotatingLog`, `HandleInterrupt`, `Accumulator`,
//! HTTP `Accept`-header content-negotiation parsing) is socket/threading/
//! logging plumbing for calibre's own hand-rolled event loop -- see
//! `crate` root doc for why `axum`/`tokio` replace that wholesale rather
//! than being ported line-by-line. What's genuinely reusable pure logic
//! is here: pagination ([`Offsets`]) and HTTP date formatting
//! ([`http_date`]).

use chrono::{DateTime, Utc};

use crate::errors::ServerError;

/// Port of `http_date`: an RFC 1123 date string suitable for HTTP
/// `Date`/`Last-Modified` headers.
pub fn http_date(when: DateTime<Utc>) -> String {
    when.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// Port of `Offsets`: pagination math for a paginated view of `total`
/// items, `delta` per page, starting at `offset`.
#[derive(Debug, Clone, Copy)]
pub struct Offsets {
    pub offset: i64,
    pub slice_upper_bound: i64,
    pub next_offset: i64,
    pub previous_offset: i64,
    pub last_offset: i64,
}

impl Offsets {
    /// Errors with [`ServerError::NotFound`] if `offset >= total`,
    /// matching upstream exactly -- including for `total == 0`, which
    /// upstream's own callers always guard against separately before
    /// constructing an `Offsets` (e.g. `get_acquisition_feed`'s own
    /// `if not ids: raise HTTPNotFound(...)`), so this preserves rather
    /// than "fixes" what looks like an edge case at first glance.
    pub fn new(offset: i64, delta: i64, total: i64) -> Result<Offsets, ServerError> {
        let offset = offset.max(0);
        if offset >= total {
            return Err(ServerError::NotFound(format!("Invalid offset: {offset}")));
        }
        let last_allowed_index = total - 1;
        let last_current_index = offset + delta - 1;
        let mut next_offset = last_current_index + 1;
        if next_offset > last_allowed_index {
            next_offset = -1;
        }
        let previous_offset = (offset - delta).max(0);
        let last_offset = (last_allowed_index - delta).max(0);
        Ok(Offsets { offset, slice_upper_bound: offset + delta, next_offset, previous_offset, last_offset })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn http_date_formats_rfc1123() {
        let dt = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        assert_eq!(http_date(dt), "Sun, 30 Aug 2026 12:00:00 GMT");
    }

    #[test]
    fn offsets_computes_next_and_last_for_a_middle_page() {
        let o = Offsets::new(10, 5, 100).unwrap();
        assert_eq!(o.offset, 10);
        assert_eq!(o.slice_upper_bound, 15);
        assert_eq!(o.next_offset, 15);
        assert_eq!(o.previous_offset, 5);
        assert_eq!(o.last_offset, 94);
    }

    #[test]
    fn offsets_next_offset_is_negative_one_on_the_last_page() {
        let o = Offsets::new(0, 50, 30).unwrap();
        assert_eq!(o.next_offset, -1);
    }

    #[test]
    fn offsets_errors_when_offset_is_out_of_range() {
        assert!(Offsets::new(100, 10, 30).is_err());
        assert!(Offsets::new(0, 10, 0).is_err());
    }

    #[test]
    fn offsets_clamps_a_negative_offset_to_zero() {
        let o = Offsets::new(-5, 10, 30).unwrap();
        assert_eq!(o.offset, 0);
    }
}
