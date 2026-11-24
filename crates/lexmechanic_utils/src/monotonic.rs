use std::time::Instant;
use lazy_static::lazy_static;

lazy_static! {
    static ref START: Instant = Instant::now();
}

/// Returns a monotonic clock value in seconds.
/// The origin is unspecified but consistent for the process.
pub fn monotonic() -> f64 {
    START.elapsed().as_secs_f64()
}
