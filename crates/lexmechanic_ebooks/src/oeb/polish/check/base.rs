//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/base.py`.
//!
//! # Design decision: one struct + a closure, not forty subclasses
//!
//! Python's `check/` package defines roughly forty `BaseError`
//! subclasses (one per distinct problem `run_checks` can report), each
//! overriding `HELP`/`INDIVIDUAL_FIX`/`level` as class attributes and,
//! for the ones that are auto-fixable, a `__call__(self, container)`
//! bound method that closes over whatever instance state it needs (a
//! duplicate id, a bad href, a corrected filename, ...).
//!
//! Every caller of these objects outside the class bodies themselves
//! (`main.py`'s `fix_errors`, the GUI's check-book dialog) only ever
//! touches the common `BaseError` surface: `.msg`/`.name`/`.line`/
//! `.col`/`.level`/`.HELP`/`.INDIVIDUAL_FIX`/`.has_multiple_locations`/
//! `.all_locations`/`getattr(e, 'is_parsing_error', False)`/
//! `getattr(e, 'FIXABLE_CSS_ERROR', False)`, plus calling the instance
//! itself when it has a fixer. Nothing outside a subclass's own
//! `__call__` ever reaches into a subclass-specific field. That means a
//! Rust translation doesn't need forty distinct struct types (or a
//! forty-variant enum carrying each one's extra fields, which would just
//! re-derive the same information the closure approach captures for
//! free): a single [`CheckError`] struct covers every common field, and
//! `INDIVIDUAL_FIX`'s bound-method-with-closed-over-state becomes
//! exactly what Rust already has a first-class construct for -- an
//! `Option<Box<dyn FnOnce(&mut Container) -> Result<bool>>>` capturing
//! whatever the fix needs by `move`. Each `check/*.rs` file builds these
//! with [`CheckError::new`] plus the builder methods below, one call
//! site per Python subclass constructor; `type_name` carries the
//! subclass's original name for identification/logging/tests without
//! needing a real type per error kind.
use std::fmt;

use anyhow::Result;

use super::super::container::Container;

/// Port of `DEBUG, INFO, WARN, ERROR, CRITICAL = range(5)`, as a proper
/// enum (`docs/AGENT_PORTING_GUIDE.md` #2: prefer enums for closed sets)
/// with the same ordering so `level > Level::Warn` comparisons (used by
/// `main.py`'s `run_checks` to short-circuit on anything worse than a
/// warning) translate directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Critical => "CRITICAL",
        };
        f.write_str(s)
    }
}

/// One entry of `BaseError.all_locations`: `(name, line, col)`.
pub type Location = (String, Option<u32>, Option<u32>);

/// A fixer closure: port of a `BaseError` subclass's `__call__(self,
/// container)`. Returns whether it actually changed anything (Python's
/// convention, preserved by [`CheckError::apply_fix`]: `None`/anything
/// other than a literal `False` counts as "changed").
pub type FixFn = Box<dyn FnOnce(&mut Container) -> Result<bool> + Send>;

/// Port of `BaseError`. See the module docs for why this one struct
/// (plus an optional fixer closure) stands in for Python's forty-odd
/// subclasses.
pub struct CheckError {
    /// The original Python subclass name (`"DuplicateId"`,
    /// `"MissingSection"`, ...) -- not read by any real Python call
    /// site, but useful here for logging/tests without needing a real
    /// type per error kind.
    pub type_name: &'static str,
    pub msg: String,
    pub name: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
    pub level: Level,
    pub help: String,
    /// `Some(label)` when this error is auto-fixable (Python's
    /// `INDIVIDUAL_FIX` being a non-empty string); the actual fixer is
    /// [`CheckError::fix`].
    pub individual_fix: Option<String>,
    pub has_multiple_locations: bool,
    pub all_locations: Option<Vec<Location>>,
    /// `getattr(e, 'is_parsing_error', False)` in Python.
    pub is_parsing_error: bool,
    /// `getattr(e, 'FIXABLE_CSS_ERROR', False)` in Python (`css.py`'s
    /// three error kinds only).
    pub fixable_css_error: bool,
    fix: Option<FixFn>,
}

impl fmt::Debug for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckError")
            .field("type_name", &self.type_name)
            .field("msg", &self.msg)
            .field("name", &self.name)
            .field("line", &self.line)
            .field("col", &self.col)
            .field("level", &self.level)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for CheckError {
    /// Port of `BaseError.__str__`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} ({}, {}):{}",
            self.type_name,
            self.name,
            self.line.map(|l| l.to_string()).unwrap_or_default(),
            self.col.map(|c| c.to_string()).unwrap_or_default(),
            self.msg
        )
    }
}

impl CheckError {
    /// Port of `BaseError.__init__`. Defaults match the class
    /// attributes: `level = ERROR`, `HELP = ''`, `INDIVIDUAL_FIX = ''`
    /// (mapped to `None`), `has_multiple_locations = False`.
    pub fn new(type_name: &'static str, msg: impl Into<String>, name: impl Into<String>) -> Self {
        CheckError {
            type_name,
            msg: msg.into(),
            name: name.into(),
            line: None,
            col: None,
            level: Level::Error,
            help: String::new(),
            individual_fix: None,
            has_multiple_locations: false,
            all_locations: None,
            is_parsing_error: false,
            fixable_css_error: false,
            fix: None,
        }
    }

    pub fn at(mut self, line: Option<u32>, col: Option<u32>) -> Self {
        self.line = line;
        self.col = col;
        self
    }

    pub fn with_level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }

    pub fn parsing_error(mut self) -> Self {
        self.is_parsing_error = true;
        self
    }

    pub fn fixable_css(mut self) -> Self {
        self.fixable_css_error = true;
        self
    }

    pub fn with_locations(mut self, locs: Vec<Location>) -> Self {
        self.has_multiple_locations = true;
        self.all_locations = Some(locs);
        self
    }

    /// Sets `INDIVIDUAL_FIX` (the human-readable label) and the fixer
    /// closure together -- in Python these are always set together (a
    /// class either has both a real `INDIVIDUAL_FIX` string and a
    /// `__call__`, or neither).
    pub fn with_fix(
        mut self,
        label: impl Into<String>,
        fix: impl FnOnce(&mut Container) -> Result<bool> + Send + 'static,
    ) -> Self {
        self.individual_fix = Some(label.into());
        self.fix = Some(Box::new(fix));
        self
    }

    pub fn is_fixable(&self) -> bool {
        self.fix.is_some()
    }

    /// Port of `err(container)`: runs this error's fixer, if it has
    /// one. Matches Python's "assume changed unless the fixer
    /// explicitly returns `False`" convention is *not* reproduced here
    /// -- every fixer in this port returns a real `bool` -- but the
    /// call-and-consume shape (a fixer only ever runs once) is the same.
    pub fn apply_fix(&mut self, container: &mut Container) -> Result<bool> {
        match self.fix.take() {
            Some(f) => f(container),
            None => Ok(false),
        }
    }
}

/// Port of `run_checkers`: runs `func` over every item in `items`,
/// concatenating the resulting error lists. Python uses a
/// `multiprocessing.pool.ThreadPool` sized to `detect_ncpus()`; this
/// port uses `std::thread::scope` with a bounded worker count, matching
/// the concurrent work-queue precedent in `oeb::polish::download`/
/// `oeb::polish::images` rather than Python's thread-pool-map shape
/// (behaviorally equivalent: every item is still processed exactly
/// once, errors are still collected into one flat list, and a panic
/// inside `func` still propagates out of this call instead of being
/// silently swallowed -- Python's `worker` wrapper re-raises via
/// `raise Exception(f'Failed to run worker: ...')`, which `thread::scope`
/// achieves for free since a spawned thread's panic re-panics the
/// scope on join).
pub fn run_checkers<T, F>(items: &[T], func: F) -> Vec<CheckError>
where
    T: Sync,
    F: Fn(&T) -> Vec<CheckError> + Sync,
{
    if items.is_empty() {
        return Vec::new();
    }
    let num_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(items.len())
        .max(1);
    let next_index = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..num_workers)
            .map(|_| {
                let next_index = &next_index;
                let items = &items;
                let func = &func;
                scope.spawn(move || {
                    let mut local = Vec::new();
                    loop {
                        let idx = next_index.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if idx >= items.len() {
                            break;
                        }
                        local.extend(func(&items[idx]));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("checker worker thread panicked"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_python_str_format() {
        let err = CheckError::new("DuplicateId", "Duplicate id: x", "chap1.html").at(Some(5), None);
        assert_eq!(
            err.to_string(),
            "DuplicateId:chap1.html (5, ):Duplicate id: x"
        );
    }

    #[test]
    fn level_ordering_matches_python_range() {
        assert!(Level::Error > Level::Warn);
        assert!(Level::Warn > Level::Info);
        assert!(Level::Critical > Level::Error);
    }

    #[test]
    fn apply_fix_runs_closure_once_and_consumes_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <manifest/><spine/>
</package>"#,
        )
        .unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        let mut err =
            CheckError::new("Fake", "msg", "content.opf").with_fix("fix it", |_container| Ok(true));
        assert!(err.is_fixable());
        assert!(err.apply_fix(&mut c).unwrap());
        // The closure was consumed; a second call is a no-op.
        assert!(!err.apply_fix(&mut c).unwrap());
    }

    #[test]
    fn run_checkers_collects_from_all_items() {
        let items = vec![1u32, 2, 3, 4, 5];
        let errors = run_checkers(&items, |n| {
            if n % 2 == 0 {
                vec![CheckError::new("Even", format!("{n} is even"), "x")]
            } else {
                vec![]
            }
        });
        assert_eq!(errors.len(), 2);
    }
}
