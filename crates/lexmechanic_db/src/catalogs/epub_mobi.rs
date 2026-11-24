//! Port of `epub_mobi.py`'s `EPUB_MOBI.run` -- but only the handful of
//! genuinely reusable, pure option-resolution steps it performs before
//! constructing a `CatalogBuilder` and calling `build_sources()`. Those
//! steps are what [`build::build_catalog`](super::build::build_catalog)
//! needs a caller to have already run to produce a
//! [`build::CatalogBuildOptions`](super::build::CatalogBuildOptions).
//!
//! # What's deliberately not ported here
//!
//! - **`cli_options`** (the `Option(...)` list of `--catalog-title`,
//!   `--generate-authors`, etc.): CLI argument definitions for
//!   `calibre.customize.CatalogPlugin`'s own plugin-registry/argparse
//!   machinery, which this crate has no equivalent of (see `mod.rs`'s
//!   own doc for the same gap). This crate's actual catalog CLI (if/when
//!   one exists) would define its own `clap`-based flags directly.
//! - **`eval()`-parsing `prefix_rules`/`exclusion_rules` CLI strings**,
//!   plus the rule-arity validation `run()` does immediately after: a
//!   Rust CLI wouldn't accept a stringified Python tuple literal to
//!   `eval()` in the first place, and [`super::epub_mobi_builder::PrefixRule`]/
//!   `get_prefix_rules`'s `(String, String, String, String)` tuple type
//!   (three-tuples for exclusion rules) already makes the arity check
//!   redundant -- the type system enforces it.
//! - **Connected-device detection and all `build_log`
//!   string-accumulation** ("Sections: ...", mount points, etc.): purely
//!   descriptive logging text with no effect on the generated catalog.
//! - **Existing-cover search (`db.search`/`db.cover`) and synthesized-cover
//!   generation (`calibre_cover2`)**: not ported -- this crate has no
//!   catalog-cover synthesis anywhere yet.
//! - **The final `Plumber` ebook-conversion run** (`.opf` -> actual
//!   `.epub`/`.mobi`/`.azw3` file): that's the `ebooks/conversion`
//!   subsystem, an entirely different, not-yet-ported part of calibre.
//!   [`build::build_catalog`](super::build::build_catalog) stops exactly
//!   where upstream's own `catalog.build_sources()` call stops: a
//!   directory of HTML/OPF/NCX content ready for that conversion step.

use chrono::Datelike;

/// Which of the six catalog sections to generate -- upstream's
/// `opts.generate_authors`/`generate_titles`/etc., bundled together
/// since [`resolve_sections`] validates and defaults them as a group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionFlags {
    pub generate_authors: bool,
    pub generate_titles: bool,
    pub generate_series: bool,
    pub generate_genres: bool,
    pub generate_recently_added: bool,
    pub generate_descriptions: bool,
}

impl SectionFlags {
    fn any(&self) -> bool {
        self.generate_authors
            || self.generate_titles
            || self.generate_series
            || self.generate_genres
            || self.generate_recently_added
            || self.generate_descriptions
    }

    fn all() -> SectionFlags {
        SectionFlags {
            generate_authors: true,
            generate_titles: true,
            generate_series: true,
            generate_genres: true,
            generate_recently_added: true,
            generate_descriptions: true,
        }
    }
}

/// Port of `run`'s "no Section switches specified" fallback and its
/// MOBI-with-only-Descriptions special case.
///
/// In a CLI environment with no sections requested, every section is
/// enabled (matching upstream's own CLI fallback). In a non-CLI
/// (upstream: GUI) environment with no sections requested, upstream
/// aborts catalog generation entirely with an error message instead --
/// returned here as `Err` for the caller to surface however it likes.
pub fn resolve_sections(mut flags: SectionFlags, fmt: &str, cli_environment: bool) -> Result<SectionFlags, String> {
    if !flags.any() {
        if cli_environment {
            flags = SectionFlags::all();
        } else {
            return Err("No enabled Sections.\nCheck E-book options tab\n'Included sections'\n".to_string());
        }
    }

    let only_descriptions = flags.generate_descriptions
        && !flags.generate_authors
        && !flags.generate_titles
        && !flags.generate_series
        && !flags.generate_genres
        && !flags.generate_recently_added;
    if fmt == "mobi" && only_descriptions {
        flags.generate_authors = true;
    }

    Ok(flags)
}

/// Port of the `if opts.exclude_genre.strip() == '':` substitution: an
/// empty `--exclude-genre` regex is upstream's shorthand for "use every
/// tag as a genre" (`'a^'`, a pattern that can never match).
pub fn resolve_exclude_genre(raw: &str) -> String {
    if raw.trim().is_empty() {
        "a^".to_string()
    } else {
        raw.to_string()
    }
}

/// Port of the `--thumb-width` clamping loop: parses `raw`, clamps to
/// `[THUMB_SMALLEST, THUMB_LARGEST]`, and falls back to
/// `THUMB_SMALLEST` on a parse failure -- matching upstream's own
/// `except Exception: ... opts.thumb_width = '1.0'` (not the in-range
/// clamp branches, which log-and-clamp instead of falling back).
pub fn resolve_thumb_width(raw: &str) -> f64 {
    const THUMB_SMALLEST: f64 = 1.0;
    const THUMB_LARGEST: f64 = 3.0;
    match raw.parse::<f64>() {
        Ok(w) => w.clamp(THUMB_SMALLEST, THUMB_LARGEST),
        Err(_) => THUMB_SMALLEST,
    }
}

/// [`resolve_output_profile`]'s output -- the resolved profile name plus
/// the description/author clip lengths upstream derives from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfile {
    pub output_profile: String,
    pub description_clip: usize,
    pub author_clip: usize,
}

/// Port of `run`'s output-profile finalization: a connected Kindle
/// overrides `opts.output_profile` entirely (`kindle_dx` for a `B004`/
/// `B005`-prefixed serial, else plain `kindle`), and the resolved
/// profile then determines `description_clip`/`author_clip`.
///
/// `output_profile` is upstream's `opts.output_profile` (`None` becomes
/// `"default"`, matching `if op is None: op = 'default'`).
/// `connected_kindle_name`/`connected_serial` are upstream's
/// `opts.connected_device['name']`/`['serial']` -- this crate has no
/// device-connection detection of its own yet (see this module's doc),
/// so callers with no such state should just pass `None`.
pub fn resolve_output_profile(
    output_profile: Option<&str>,
    connected_kindle_name: Option<&str>,
    connected_serial: Option<&str>,
) -> ResolvedProfile {
    let mut op = output_profile.unwrap_or("default").to_string();

    if connected_kindle_name.is_some_and(|n| n.to_lowercase().contains("kindle")) {
        op = match connected_serial {
            Some(serial) if serial.len() >= 4 && matches!(&serial[..4], "B004" | "B005") => "kindle_dx".to_string(),
            _ => "kindle".to_string(),
        };
    }

    let use_wide_clips = op.ends_with("dx") || !op.contains("kindle");
    let (description_clip, author_clip) = if use_wide_clips { (380, 100) } else { (100, 60) };

    ResolvedProfile { output_profile: op, description_clip, author_clip }
}

/// Port of `opts.creator = '{}, {} {}, {}'.format(strftime('%A'),
/// strftime('%B'), strftime('%d').lstrip('0'), strftime('%Y'))`, e.g.
/// `"Saturday, August 29, 2026"`. `now` replaces upstream's implicit
/// wall-clock read, matching this whole port's "caller supplies
/// impure/environment-derived inputs" convention.
pub fn format_creator(now: chrono::DateTime<chrono::Utc>) -> String {
    format!("{}, {} {}, {}", now.format("%A"), now.format("%B"), now.day(), now.format("%Y"))
}

/// Port of `opts.creator_sort_as = '{} {}'.format('calibre',
/// strftime('%Y-%m-%d'))`, e.g. `"calibre 2026-08-29"`.
pub fn format_creator_sort_as(now: chrono::DateTime<chrono::Utc>) -> String {
    format!("calibre {}", now.format("%Y-%m-%d"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // --- resolve_sections ---

    #[test]
    fn resolve_sections_enables_everything_when_none_requested_in_a_cli_environment() {
        let flags = resolve_sections(SectionFlags::default(), "epub", true).unwrap();
        assert_eq!(flags, SectionFlags::all());
    }

    #[test]
    fn resolve_sections_errors_when_none_requested_outside_a_cli_environment() {
        let err = resolve_sections(SectionFlags::default(), "epub", false).unwrap_err();
        assert!(err.contains("No enabled Sections"));
    }

    #[test]
    fn resolve_sections_leaves_an_explicit_selection_alone() {
        let flags = SectionFlags { generate_titles: true, ..Default::default() };
        let resolved = resolve_sections(flags, "epub", true).unwrap();
        assert_eq!(resolved, flags);
    }

    #[test]
    fn resolve_sections_forces_authors_for_mobi_with_only_descriptions() {
        let flags = SectionFlags { generate_descriptions: true, ..Default::default() };
        let resolved = resolve_sections(flags, "mobi", true).unwrap();
        assert!(resolved.generate_authors);
        assert!(resolved.generate_descriptions);
    }

    #[test]
    fn resolve_sections_does_not_force_authors_for_epub_with_only_descriptions() {
        let flags = SectionFlags { generate_descriptions: true, ..Default::default() };
        let resolved = resolve_sections(flags, "epub", true).unwrap();
        assert!(!resolved.generate_authors);
    }

    #[test]
    fn resolve_sections_does_not_force_authors_when_other_sections_already_present() {
        let flags = SectionFlags { generate_descriptions: true, generate_titles: true, ..Default::default() };
        let resolved = resolve_sections(flags, "mobi", true).unwrap();
        assert!(!resolved.generate_authors);
    }

    // --- resolve_exclude_genre ---

    #[test]
    fn resolve_exclude_genre_passes_through_a_non_empty_pattern() {
        assert_eq!(resolve_exclude_genre(r"\[.+\]"), r"\[.+\]");
    }

    #[test]
    fn resolve_exclude_genre_substitutes_an_unmatchable_pattern_when_blank() {
        assert_eq!(resolve_exclude_genre(""), "a^");
        assert_eq!(resolve_exclude_genre("   "), "a^");
    }

    // --- resolve_thumb_width ---

    #[test]
    fn resolve_thumb_width_passes_through_an_in_range_value() {
        assert_eq!(resolve_thumb_width("1.5"), 1.5);
    }

    #[test]
    fn resolve_thumb_width_clamps_below_the_smallest() {
        assert_eq!(resolve_thumb_width("0.2"), 1.0);
    }

    #[test]
    fn resolve_thumb_width_clamps_above_the_largest() {
        assert_eq!(resolve_thumb_width("10"), 3.0);
    }

    #[test]
    fn resolve_thumb_width_falls_back_to_smallest_on_a_parse_failure() {
        assert_eq!(resolve_thumb_width("not-a-number"), 1.0);
    }

    // --- resolve_output_profile ---

    #[test]
    fn resolve_output_profile_defaults_to_default_when_unset() {
        let r = resolve_output_profile(None, None, None);
        assert_eq!(r.output_profile, "default");
        assert_eq!(r.description_clip, 380);
        assert_eq!(r.author_clip, 100);
    }

    #[test]
    fn resolve_output_profile_uses_narrow_clips_for_a_plain_kindle_profile() {
        let r = resolve_output_profile(Some("kindle"), None, None);
        assert_eq!(r.output_profile, "kindle");
        assert_eq!(r.description_clip, 100);
        assert_eq!(r.author_clip, 60);
    }

    #[test]
    fn resolve_output_profile_uses_wide_clips_for_a_dx_profile() {
        let r = resolve_output_profile(Some("kindle_dx"), None, None);
        assert_eq!(r.description_clip, 380);
        assert_eq!(r.author_clip, 100);
    }

    #[test]
    fn resolve_output_profile_overrides_to_kindle_dx_for_a_b004_serial() {
        let r = resolve_output_profile(Some("default"), Some("Kindle"), Some("B004123456"));
        assert_eq!(r.output_profile, "kindle_dx");
    }

    #[test]
    fn resolve_output_profile_overrides_to_plain_kindle_for_an_unrecognized_serial() {
        let r = resolve_output_profile(Some("default"), Some("Kindle"), Some("XXXX123456"));
        assert_eq!(r.output_profile, "kindle");
    }

    #[test]
    fn resolve_output_profile_ignores_a_non_kindle_connected_device() {
        let r = resolve_output_profile(Some("nook"), Some("Nook"), None);
        assert_eq!(r.output_profile, "nook");
    }

    // --- creator formatting ---

    #[test]
    fn format_creator_matches_the_expected_shape() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 8, 29, 12, 0, 0).unwrap();
        assert_eq!(format_creator(now), "Saturday, August 29, 2026");
    }

    #[test]
    fn format_creator_drops_the_leading_zero_on_single_digit_days() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap();
        assert_eq!(format_creator(now), "Wednesday, August 5, 2026");
    }

    #[test]
    fn format_creator_sort_as_matches_the_expected_shape() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 8, 29, 12, 0, 0).unwrap();
        assert_eq!(format_creator_sort_as(now), "calibre 2026-08-29");
    }
}
