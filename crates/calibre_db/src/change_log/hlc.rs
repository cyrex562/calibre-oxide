//! Hybrid logical clocks, for ordering changes made on machines that
//! never talk to each other.
//!
//! # Why not wall time
//!
//! The change log is merged by file sync: two machines write into
//! `.calibre-oxide/changes/` and the sync service delivers each one's
//! files to the other. Nothing coordinates them, so the only thing
//! that can order their changes is what each entry carries.
//!
//! Wall time alone is the obvious choice and is wrong. Clocks drift,
//! and a machine whose clock is five minutes fast wins *every*
//! conflict for as long as it stays fast -- so an edit made later on
//! the correct machine is silently discarded in favour of an earlier
//! one. Worse, it is undetectable after the fact: the log looks
//! perfectly consistent.
//!
//! # What an HLC does instead
//!
//! A hybrid logical clock (Kulkarni et al., 2014) keeps wall time as
//! its coarse component and a counter as its fine one, and pulls the
//! wall component *forward* whenever it observes a stamp from ahead of
//! it. The result: stamps are strictly increasing locally, they stay
//! close to real time, and a peer that has seen your change can never
//! mint a stamp that sorts before it. Causality is respected even
//! though the clocks are not synchronised.
//!
//! It does not give a total order on its own -- two peers can mint
//! equal stamps -- so the log breaks ties on the origin id, which is
//! arbitrary but identical on every machine. See [`Hlc::cmp`].

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A point in the merged history of a library.
///
/// Ordered by `(wall_ms, counter)`. Deliberately *not* a total order on
/// its own: two peers can produce identical stamps, and the log
/// resolves that by also comparing the origin id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hlc {
    /// Milliseconds since the Unix epoch, as the minting machine saw
    /// it -- except when it has been dragged forward by a peer's
    /// stamp, which is the whole point.
    pub wall_ms: u64,
    /// Distinguishes stamps minted within the same millisecond, and
    /// keeps the clock strictly increasing when `wall_ms` cannot move
    /// (a coarse system clock, or a stamp already pulled ahead of real
    /// time).
    pub counter: u32,
}

impl Hlc {
    pub const ZERO: Hlc = Hlc { wall_ms: 0, counter: 0 };

    /// Formats as the fixed-width, lexicographically sortable prefix a
    /// change filename starts with.
    ///
    /// Fixed width matters: `9-0` sorts after `10-0` as text, so an
    /// unpadded stamp would make the directory listing order disagree
    /// with the clock order — and reading the log in filename order is
    /// the cheap path this exists to enable.
    ///
    /// 13 digits covers milliseconds until the year 2286.
    pub fn file_prefix(&self) -> String {
        format!("{:013}-{:05}", self.wall_ms, self.counter)
    }
}

/// A monotonic HLC source for one process.
///
/// Not `Copy`/`Clone`: there must be exactly one per library so that
/// two stamps minted in the same process cannot collide.
#[derive(Debug)]
pub struct HlcClock {
    last: Hlc,
}

impl HlcClock {
    /// Starts from a stamp already known to exist -- the tip of this
    /// install's own log.
    ///
    /// Resuming from the tip rather than from zero is what stops a
    /// restart re-minting stamps it has already used, which would
    /// collide with its own history on disk.
    pub fn resuming_from(last: Hlc) -> Self {
        HlcClock { last }
    }

    /// The next stamp, strictly greater than every stamp this clock has
    /// minted or observed.
    pub fn now(&mut self) -> Hlc {
        self.tick(physical_now_ms())
    }

    /// Pulls the clock forward past a stamp seen from another machine,
    /// so anything minted afterwards sorts after it.
    ///
    /// Called for every entry read during a merge. Without it, a peer's
    /// change from the future would keep winning against local edits
    /// made after it was merged -- the user's most recent edit losing
    /// to one they already saw.
    pub fn observe(&mut self, remote: Hlc) {
        if remote > self.last {
            self.last = remote;
        }
    }

    /// The clock's current tip, without advancing it.
    pub fn peek(&self) -> Hlc {
        self.last
    }

    /// Split out from [`HlcClock::now`] so the stepping rule can be
    /// tested against a supplied physical time instead of the real
    /// clock.
    fn tick(&mut self, physical_ms: u64) -> Hlc {
        let next = if physical_ms > self.last.wall_ms {
            // Real time has moved on: follow it and reset the counter,
            // which keeps stamps close to wall time rather than
            // drifting ever further ahead.
            Hlc { wall_ms: physical_ms, counter: 0 }
        } else {
            // Real time has not moved on, or has gone *backwards*
            // (NTP correction, a laptop resuming, a VM snapshot). Hold
            // the wall component and step the counter: the clock must
            // never go backwards, even when the system clock does.
            Hlc { wall_ms: self.last.wall_ms, counter: self.last.counter.saturating_add(1) }
        };
        self.last = next;
        next
    }
}

fn physical_now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_follow_real_time_when_it_moves() {
        let mut clock = HlcClock::resuming_from(Hlc::ZERO);
        assert_eq!(clock.tick(1_000), Hlc { wall_ms: 1_000, counter: 0 });
        assert_eq!(clock.tick(1_001), Hlc { wall_ms: 1_001, counter: 0 });
    }

    #[test]
    fn stamps_step_the_counter_within_one_millisecond() {
        let mut clock = HlcClock::resuming_from(Hlc::ZERO);
        assert_eq!(clock.tick(1_000), Hlc { wall_ms: 1_000, counter: 0 });
        assert_eq!(clock.tick(1_000), Hlc { wall_ms: 1_000, counter: 1 });
        assert_eq!(clock.tick(1_000), Hlc { wall_ms: 1_000, counter: 2 });
    }

    /// NTP corrections, laptops resuming and restored VM snapshots all
    /// move the system clock backwards. A log whose stamps went
    /// backwards with it would order later edits before earlier ones.
    #[test]
    fn the_clock_never_goes_backwards_when_the_system_clock_does() {
        let mut clock = HlcClock::resuming_from(Hlc::ZERO);
        let first = clock.tick(5_000);
        let second = clock.tick(1_000);
        assert!(second > first, "{second:?} should be after {first:?}");
        assert_eq!(second.wall_ms, 5_000);
    }

    /// The case wall time alone gets wrong: a peer five minutes fast
    /// would otherwise win every conflict indefinitely.
    #[test]
    fn observing_a_peer_from_the_future_drags_the_clock_forward() {
        let mut clock = HlcClock::resuming_from(Hlc::ZERO);
        clock.tick(1_000);
        clock.observe(Hlc { wall_ms: 300_000, counter: 7 });

        // Real time is still 1_001, but the next local stamp must sort
        // after the peer's -- otherwise an edit made now would lose to
        // one the user has already seen.
        let next = clock.tick(1_001);
        assert!(next > Hlc { wall_ms: 300_000, counter: 7 });
        assert_eq!(next, Hlc { wall_ms: 300_000, counter: 8 });
    }

    #[test]
    fn observing_a_peer_from_the_past_changes_nothing() {
        let mut clock = HlcClock::resuming_from(Hlc { wall_ms: 9_000, counter: 0 });
        clock.observe(Hlc { wall_ms: 1, counter: 1 });
        assert_eq!(clock.peek(), Hlc { wall_ms: 9_000, counter: 0 });
    }

    /// A restart must not re-mint stamps that are already on disk.
    #[test]
    fn resuming_continues_past_the_recorded_tip() {
        let tip = Hlc { wall_ms: 5_000, counter: 3 };
        let mut clock = HlcClock::resuming_from(tip);
        assert!(clock.tick(4_000) > tip);
    }

    #[test]
    fn file_prefixes_sort_in_clock_order() {
        let mut stamps = vec![
            Hlc { wall_ms: 10, counter: 0 },
            Hlc { wall_ms: 9, counter: 0 },
            Hlc { wall_ms: 9, counter: 11 },
            Hlc { wall_ms: 9, counter: 2 },
        ];
        stamps.sort();

        let by_clock: Vec<String> = stamps.iter().map(Hlc::file_prefix).collect();
        let mut by_text = by_clock.clone();
        by_text.sort();
        // Unpadded, "9-11" would sort before "9-2" and both before
        // "10-0"; padded, filename order is clock order.
        assert_eq!(by_clock, by_text);
    }

    #[test]
    fn a_real_clock_produces_strictly_increasing_stamps() {
        let mut clock = HlcClock::resuming_from(Hlc::ZERO);
        let stamps: Vec<Hlc> = (0..1_000).map(|_| clock.now()).collect();
        for pair in stamps.windows(2) {
            assert!(pair[1] > pair[0], "{:?} then {:?}", pair[0], pair[1]);
        }
    }
}
