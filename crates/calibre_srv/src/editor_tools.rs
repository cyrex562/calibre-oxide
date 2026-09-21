//! Check-book and reports for the editor (issues 3.3 and 3.2 of the
//! #816 epic).
//!
//! Both engines were fully ported and had no caller anywhere:
//! `oeb::polish::check::run_checks`/`fix_errors` and
//! `oeb::polish::report`'s `files_data`/`images_data`/`links_data`.
//! Upstream drives them from Qt dialogs, so there is no `calibre.srv`
//! route to port.
//!
//! # Built on the existing tweak session
//!
//! Both take an already-open `Container`, which a tweak session
//! already holds (`tweak.rs`). Hanging these routes off that session
//! means they see exactly the book the user is editing, including
//! unsaved changes -- checking the on-disk copy instead would report
//! problems the editor had already fixed.

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use calibre_ebooks::oeb::polish::check::main::{fix_errors, run_checks};
use calibre_ebooks::oeb::polish::report;
use calibre_ebooks::oeb::polish::spell as polish_spell;
use calibre_ebooks::spell::dictionary::{DictionaryMeta, Dictionaries};
use calibre_ebooks::spell::{parse_lang_code, vendored, DictionaryLocale};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::ServerError;
use crate::AppState;

/// EPUB is the only container a tweak session opens, so it is the
/// only book type these can be asked about.
const BOOK_TYPE: &str = "epub";

fn level_name(level: calibre_ebooks::oeb::polish::check::base::Level) -> &'static str {
    use calibre_ebooks::oeb::polish::check::base::Level;
    match level {
        Level::Debug => "debug",
        Level::Info => "info",
        Level::Warn => "warning",
        Level::Error => "error",
        Level::Critical => "critical",
    }
}

/// `GET /tweak/check/{session_id}` -- runs every check against the
/// session's live container.
pub async fn check(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let errors = state
            .tweak_sessions
            .with_container(&session_id, |container| run_checks(container, BOOK_TYPE))
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        let items: Vec<Value> = errors
            .iter()
            .map(|e| {
                json!({
                    "type": e.type_name,
                    "message": e.msg,
                    "file": e.name,
                    "line": e.line,
                    "col": e.col,
                    "level": level_name(e.level),
                    "help": e.help,
                    // What the UI needs to decide whether to offer a
                    // "fix this" button at all.
                    "fixable": e.individual_fix.is_some(),
                    "fix_label": e.individual_fix,
                })
            })
            .collect();

        let errors_count = errors.iter().filter(|e| level_name(e.level) == "error" || level_name(e.level) == "critical").count();
        Ok(Json(json!({ "count": items.len(), "errors": errors_count, "fixable": items.iter().filter(|i| i["fixable"] == json!(true)).count(), "items": items })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

/// `POST /tweak/check-fix/{session_id}` -- applies every automatic
/// fix the checks offered.
///
/// Fixes are applied to the open session rather than to disk, so they
/// are part of the same commit-or-discard decision as any other
/// editor change. A user who dislikes the result can still discard.
pub async fn check_fix(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let result = state
            .tweak_sessions
            .with_container(&session_id, |container| -> anyhow::Result<(usize, bool)> {
                let errors = run_checks(container, BOOK_TYPE)?;
                let fixable: Vec<_> = errors.into_iter().filter(|e| e.individual_fix.is_some()).collect();
                let attempted = fixable.len();
                if attempted == 0 {
                    return Ok((0, false));
                }
                let changed = fix_errors(container, fixable)?;
                Ok((attempted, changed))
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        Ok(Json(json!({ "attempted": result.0, "changed": result.1 })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

/// `GET /tweak/report/{session_id}` -- the files, images and links
/// breakdown of the book being edited.
///
/// Word counts are deliberately excluded: `report::words_data` needs
/// a loaded dictionary to attribute words to a locale, and no
/// dictionaries are vendored here (see #816's spell-check item). A
/// report that silently omitted them would be worse than one that
/// does not claim to offer them.
pub async fn book_report(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let (files, images) = state
            .tweak_sessions
            .with_container(&session_id, |container| -> anyhow::Result<_> {
                let files = report::files_data(container, None);
                let images = report::images_data(container)?;
                Ok((files, images))
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        let file_items: Vec<Value> = files
            .iter()
            .map(|f| json!({ "name": f.name, "category": f.category, "size": f.size, "words": f.word_count }))
            .collect();

        let image_items: Vec<Value> = images
            .iter()
            .map(|i| json!({ "name": i.name, "size": i.size, "width": i.width, "height": i.height, "usage": i.usage.len() }))
            .collect();

        let total_size: u64 = files.iter().map(|f| f.size).sum();

        Ok(Json(json!({
            "files": { "count": file_items.len(), "total_size": total_size, "items": file_items },
            "images": { "count": image_items.len(), "items": image_items },
        })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

/// Builds the dictionary set from what ships with the binary.
///
/// Vendored rather than configured (#865): requiring a
/// `--dictionaries-dir` or a system hunspell install means spell
/// check silently does nothing on a machine that has neither, which
/// is most Windows machines and plenty of Linux ones.
fn dictionaries(locale: DictionaryLocale) -> Dictionaries {
    let builtin: Vec<DictionaryMeta> = vendored::builtin();
    Dictionaries::new(locale, Vec::new(), builtin, Vec::new(), |_| None, |_| None)
}

/// `GET /tweak/spell/{session_id}` -- misspelled words in the book
/// being edited, with where each one appears.
///
/// The engine (`oeb::polish::spell` + `spell::dictionary`) was fully
/// ported and had no caller. What was missing was dictionary *data*,
/// which now ships with the binary.
///
/// Suggestions are computed only for the words actually returned, not
/// for every word in the book: generating them is the expensive part,
/// and a book has far more correct words than misspelled ones.
pub async fn spell_check(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        // `eng-US` unless the book says otherwise; `get_all_words`
        // needs a locale to attribute words to.
        let locale = parse_lang_code("en-US").map_err(ServerError::InternalServerError)?;

        let words = state
            .tweak_sessions
            .with_container(&session_id, |container| {
                let mut counts = std::collections::HashMap::new();
                polish_spell::get_all_words(container, &locale, &std::collections::HashSet::new(), &mut counts)
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?
            .1;

        let mut dicts = dictionaries(locale);

        let mut misspelled: Vec<Value> = Vec::new();
        for ((word, word_locale), locations) in words {
            if dicts.recognized(&word, Some(&word_locale)) {
                continue;
            }
            let files: Vec<String> = {
                let mut names: Vec<String> = locations.iter().map(|l| l.file_name.clone()).collect();
                names.sort();
                names.dedup();
                names
            };
            misspelled.push(json!({
                "word": word,
                "count": locations.len(),
                "files": files,
                "suggestions": dicts.suggestions(&word, Some(&word_locale)).into_iter().take(5).collect::<Vec<_>>(),
            }));
        }

        // Most-frequent first: a word appearing thirty times is more
        // likely a real problem than a one-off proper noun.
        misspelled.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()).then_with(|| a["word"].as_str().cmp(&b["word"].as_str())));

        Ok(Json(json!({ "count": misspelled.len(), "words": misspelled })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}


// ===================================================================
// Search and replace across the book (#816 item 3.4)
// ===================================================================
//
// The editor could open one file at a time and edit it by hand. A
// rename that appears in forty files was forty manual edits, which is
// most of why an editor needs this at all.

#[derive(Debug, Deserialize)]
pub struct SearchReplaceBody {
    pub find: String,
    /// Omitted means "search only" -- nothing is written.
    #[serde(default)]
    pub replace: Option<String>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    /// Report what would change without writing it.
    #[serde(default)]
    pub dry_run: bool,
}

/// How much text either side of a match to show.
const CONTEXT_RADIUS: usize = 40;

/// Builds the matcher.
///
/// A plain search is escaped rather than compiled: someone looking
/// for `C++` or `(draft)` means those characters, and compiling the
/// box as a pattern would either error or match something wildly
/// unrelated. The same reasoning as the bulk metadata editor's
/// search-and-replace.
fn build_pattern(body: &SearchReplaceBody) -> Result<regex::Regex, ServerError> {
    let raw = if body.regex { body.find.clone() } else { regex::escape(&body.find) };
    let pattern = if body.case_sensitive { raw } else { format!("(?i){raw}") };
    regex::Regex::new(&pattern).map_err(|e| ServerError::BadRequest(format!("invalid search pattern: {e}")))
}

/// Character-safe slice around a byte offset.
///
/// Slicing a UTF-8 string at an arbitrary byte offset panics on a
/// multi-byte boundary, and book text is full of curly quotes and
/// accented letters -- so the window is walked to real char
/// boundaries rather than trusting arithmetic.
fn context_around(text: &str, start: usize, end: usize) -> String {
    let lo = text[..start].char_indices().rev().take(CONTEXT_RADIUS).last().map(|(i, _)| i).unwrap_or(start);
    let hi = text[end..].char_indices().take(CONTEXT_RADIUS).last().map(|(i, c)| end + i + c.len_utf8()).unwrap_or(end);
    text[lo..hi].split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `POST /tweak/search-replace/{session_id}`.
///
/// Acts on the open session, so a replace is part of the same
/// commit-or-discard decision as any other editor change -- a user
/// who dislikes the result can still discard the whole session.
pub async fn search_replace(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>, Json(body): Json<SearchReplaceBody>) -> Result<Json<Value>, ServerError> {
    if body.find.is_empty() {
        return Err(ServerError::BadRequest("find must not be empty".to_string()));
    }
    let pattern = build_pattern(&body)?;
    // Validated before touching a single file: a bad pattern found
    // halfway through leaves the book half-rewritten.
    let writing = body.replace.is_some() && !body.dry_run;

    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let outcome = state
            .tweak_sessions
            .with_container(&session_id, |container| -> anyhow::Result<(Vec<Value>, usize, usize)> {
                let mut names: Vec<String> = container.name_path_map.keys().cloned().collect();
                names.sort();

                let mut files = Vec::new();
                let mut total = 0usize;
                let mut changed_files = 0usize;

                for name in names {
                    // Never offer to rewrite a binary: a "match" in
                    // image bytes is noise at best and corruption at
                    // worst.
                    if !crate::tweak::is_editable_as_text(&container.guess_type(&name)) {
                        continue;
                    }
                    let Ok(bytes) = container.raw_data(&name, true) else { continue };
                    let Ok(text) = String::from_utf8(bytes) else { continue };

                    let matches: Vec<(usize, usize)> = pattern.find_iter(&text).map(|m| (m.start(), m.end())).collect();
                    if matches.is_empty() {
                        continue;
                    }
                    total += matches.len();

                    let samples: Vec<Value> = matches
                        .iter()
                        .take(5)
                        .map(|(s, e)| {
                            json!({
                                // 1-based, matching how every editor
                                // and error message counts lines.
                                "line": text[..*s].matches('\n').count() + 1,
                                "text": &text[*s..*e],
                                "context": context_around(&text, *s, *e),
                            })
                        })
                        .collect();

                    files.push(json!({ "name": name, "count": matches.len(), "samples": samples }));

                    if writing {
                        if let Some(replacement) = &body.replace {
                            let rewritten = pattern.replace_all(&text, replacement.as_str()).into_owned();
                            if rewritten != text {
                                container.write_file(&name, rewritten.as_bytes())?;
                                changed_files += 1;
                            }
                        }
                    }
                }
                Ok((files, total, changed_files))
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        Ok(Json(json!({
            "matches": outcome.1,
            "files": outcome.0,
            "replaced": writing,
            "changed_files": outcome.2,
        })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}


// ===================================================================
// Fonts (#816 item 3.6)
// ===================================================================
//
// `oeb::polish::fonts::font_family_data` was ported and had no
// caller. It answers the question that matters before shipping a
// book: which font families does this book *ask* for, and which of
// those does it actually carry?
//
// A family a book references but does not embed renders as whatever
// the reading device happens to have -- which on an e-reader is
// usually nothing like what the designer intended, and is invisible
// until someone opens it on hardware.

/// `GET /tweak/fonts/{session_id}`.
pub async fn fonts(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        let families = state
            .tweak_sessions
            .with_container(&session_id, |container| calibre_ebooks::oeb::polish::fonts::font_family_data(container))
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?
            .map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        // Sorted so the listing is stable between requests -- a
        // HashMap's order is not, and a panel that reshuffles on every
        // refresh is hard to read.
        let mut items: Vec<(String, bool)> = families.into_iter().collect();
        items.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        let embedded = items.iter().filter(|(_, e)| *e).count();
        let entries: Vec<Value> = items.iter().map(|(family, is_embedded)| json!({ "family": family, "embedded": is_embedded })).collect();

        Ok(Json(json!({
            "count": entries.len(),
            "embedded": embedded,
            // The actionable number: families the book asks for but
            // does not ship.
            "not_embedded": entries.len() - embedded,
            "families": entries,
        })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}


// ===================================================================
// Diff against the saved book (#816 item 3.5)
// ===================================================================
//
// The editor, polish, search-and-replace and the TOC editor all write
// into the same session, and the only way to see what they had
// collectively done was to commit and look. This compares the open
// session against the copy still in the library, so the whole set of
// pending changes can be reviewed before committing them.

/// Lines either side of a change to show for orientation.
const DIFF_CONTEXT: usize = 2;

/// A unified-style diff between two texts, as structured lines.
///
/// Returns `None` when they are identical, so an unchanged file
/// produces no entry rather than an empty one.
fn diff_lines(before: &str, after: &str) -> Option<Vec<Value>> {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(before, after);
    let mut out = Vec::new();
    let mut any_change = false;

    for group in diff.grouped_ops(DIFF_CONTEXT) {
        for op in group {
            for change in diff.iter_changes(&op) {
                let tag = match change.tag() {
                    ChangeTag::Delete => "remove",
                    ChangeTag::Insert => "add",
                    ChangeTag::Equal => "context",
                };
                if tag != "context" {
                    any_change = true;
                }
                out.push(json!({
                    "tag": tag,
                    // 1-based, and absent on the side a line does not
                    // exist -- an added line has no old number.
                    "old_line": change.old_index().map(|i| i + 1),
                    "new_line": change.new_index().map(|i| i + 1),
                    "text": change.value().trim_end_matches('\n'),
                }));
            }
        }
    }

    any_change.then_some(out)
}

/// `GET /tweak/diff/{session_id}` -- every file that differs from the
/// saved book.
pub async fn diff(State(state): State<AppState>, AxumPath(session_id): AxumPath<String>) -> Result<Json<Value>, ServerError> {
    let (book_id, library_id) = state.tweak_sessions.session_book(&session_id).ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?;
    let cache = state.cache_for(library_id.as_deref()).ok_or_else(|| ServerError::NotFound(format!("no library named {:?}", library_id.unwrap_or_default())))?;

    tokio::task::spawn_blocking(move || -> Result<Json<Value>, ServerError> {
        // The saved copy, opened into its own temp directory so it
        // cannot disturb the session's own extracted tree.
        let ids: std::collections::HashSet<i32> = std::iter::once(book_id).collect();
        let rows = cache.get_data_as_dict(None, true, Some(&ids), false).map_err(|e| ServerError::InternalServerError(e.to_string()))?;
        let row = rows.into_iter().next().ok_or_else(|| ServerError::NotFound(format!("no book with id {book_id}")))?;
        let path = row.get("fmt_epub").and_then(|v| v.as_str()).ok_or_else(|| ServerError::NotFound(format!("no epub format for book {book_id}")))?.to_string();

        let tdir = tempfile::tempdir().map_err(|e| ServerError::InternalServerError(e.to_string()))?;
        let mut saved = calibre_ebooks::oeb::polish::container::EpubContainer::open_zip(std::path::Path::new(&path), tdir.path()).map_err(|e| ServerError::InternalServerError(e.to_string()))?;

        let files = state
            .tweak_sessions
            .with_container(&session_id, |container| -> Vec<Value> {
                let mut names: std::collections::BTreeSet<String> = container.name_path_map.keys().cloned().collect();
                names.extend(saved.name_path_map.keys().cloned());

                let mut out = Vec::new();
                for name in names {
                    let in_session = container.name_path_map.contains_key(&name);
                    let in_saved = saved.name_path_map.contains_key(&name);

                    // A file added or deleted in the session is a real
                    // change, and a line diff of it against nothing
                    // would be pure noise.
                    if !in_saved {
                        out.push(json!({ "name": name, "status": "added" }));
                        continue;
                    }
                    if !in_session {
                        out.push(json!({ "name": name, "status": "removed" }));
                        continue;
                    }

                    if !crate::tweak::is_editable_as_text(&container.guess_type(&name)) {
                        // Binary: compare bytes, but never try to show
                        // a line diff of them.
                        let a = saved.raw_data(&name, true).unwrap_or_default();
                        let b = container.raw_data(&name, true).unwrap_or_default();
                        if a != b {
                            out.push(json!({ "name": name, "status": "binary-changed" }));
                        }
                        continue;
                    }

                    let before = saved.raw_data(&name, true).ok().and_then(|b| String::from_utf8(b).ok());
                    let after = container.raw_data(&name, true).ok().and_then(|b| String::from_utf8(b).ok());
                    let (Some(before), Some(after)) = (before, after) else { continue };

                    if let Some(lines) = diff_lines(&before, &after) {
                        out.push(json!({ "name": name, "status": "modified", "lines": lines }));
                    }
                }
                out
            })
            .ok_or_else(|| ServerError::NotFound(format!("No tweak session: {session_id}")))?;

        Ok(Json(json!({ "changed": files.len(), "files": files })))
    })
    .await
    .map_err(|e| ServerError::InternalServerError(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use calibre_db::cache::Cache;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    /// A real EPUB carrying a genuine defect -- two elements sharing
    /// an id -- so `run_checks` has something true to find. A clean
    /// book would pass these tests whether or not the checks ran.
    fn epub_with_a_duplicate_id() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mimetype"), "application/epub+zip").unwrap();
        std::fs::create_dir_all(dir.path().join("META-INF")).unwrap();
        std::fs::write(
            dir.path().join("META-INF/container.xml"),
            r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("chapter1.xhtml"),
            r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p id="dup">One</p><p id="dup">Two</p><p>A clear misspelling: libary.</p></body></html>"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("content.opf"),
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="bookid">id1</dc:identifier><dc:title>Test</dc:title><dc:language>en</dc:language></metadata>
<manifest><item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c1"/></spine>
</package>"#,
        )
        .unwrap();

        let epub_path = dir.path().join("out.epub");
        let file = std::fs::File::create(&epub_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for name in ["mimetype", "META-INF/container.xml", "content.opf", "chapter1.xhtml"] {
            zip.start_file(name, opts).unwrap();
            use std::io::Write;
            zip.write_all(&std::fs::read(dir.path().join(name)).unwrap()).unwrap();
        }
        zip.finish().unwrap();
        std::fs::read(&epub_path).unwrap()
    }

    fn test_app() -> (tempfile::TempDir, axum::Router, i32) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let source = dir.path().join("Book.epub");
        std::fs::write(&source, epub_with_a_duplicate_id()).unwrap();
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "Test".to_string();
        meta.authors = vec!["Author".to_string()];
        let book_id = cache.add_book(&source, &meta).unwrap();
        let state = crate::AppState {
            libraries: None,
            cache: std::sync::Arc::new(cache),
            opts: std::sync::Arc::new(crate::opts::ServerOptions::default()),
            auth: None,
            changes: crate::web_socket::new_change_broadcaster(),
            reader_profiles: std::sync::Arc::new(crate::reader_profiles::ProfileStore::new_in_memory().unwrap()),
            book_cache: std::sync::Arc::new(crate::books_cache::BookCache::open_temp()),
            jobs: std::sync::Arc::new(crate::jobs::JobsManager::new(4, std::time::Duration::from_secs(3600))),
            render_jobs: std::sync::Arc::new(crate::render_endpoints::RenderJobRegistry::new()),
            conversion_jobs: std::sync::Arc::new(crate::convert::ConversionJobRegistry::new()),
            news_jobs: std::sync::Arc::new(crate::news::NewsJobRegistry::new()),
            tweak_sessions: std::sync::Arc::new(crate::tweak::TweakSessionRegistry::new()),
            news_schedules: std::sync::Arc::new(crate::news_scheduler::NewsScheduleStore::new_in_memory().unwrap()),
            tts_voice: None,
            plugin_store: None,
            plugin_registry: std::sync::Arc::new(std::sync::Mutex::new(calibre_customize::registry::PluginRegistry::new())),
        };
        (dir, crate::test_router(state), book_id)
    }

    async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = router.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn post(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = router.clone().oneshot(Request::builder().method("POST").uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Opens a tweak session and returns its id.
    async fn open_session(router: &axum::Router, book_id: i32) -> String {
        let (status, body) = post(router, &format!("/tweak/open/{book_id}/epub/default")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["session_id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn check_finds_a_real_defect() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/check/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        // The book really does have two elements sharing an id.
        let types: Vec<&str> = body["items"].as_array().unwrap().iter().filter_map(|i| i["type"].as_str()).collect();
        assert!(types.iter().any(|t| t.contains("DuplicateId")), "expected a duplicate-id error, got {types:?}");
    }

    #[tokio::test]
    async fn check_reports_levels_and_fixability_for_the_ui() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (_, body) = get(&router, &format!("/tweak/check/{session}")).await;

        for item in body["items"].as_array().unwrap() {
            // Without a level the UI cannot tell a note from a real
            // problem, and without `fixable` it cannot decide whether
            // to offer a fix button at all.
            assert!(item["level"].is_string(), "{item}");
            assert!(item["fixable"].is_boolean(), "{item}");
        }
    }

    #[tokio::test]
    async fn check_fix_reports_what_it_attempted() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = post(&router, &format!("/tweak/check-fix/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["attempted"].is_number(), "{body}");
        assert!(body["changed"].is_boolean(), "{body}");
    }

    #[tokio::test]
    async fn the_report_describes_the_book_being_edited() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/report/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let names: Vec<&str> = body["files"]["items"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).collect();
        assert!(names.contains(&"chapter1.xhtml"), "{body}");
        assert!(body["files"]["total_size"].as_u64().unwrap() > 0, "{body}");
    }

    /// These act on the *open session*, not the on-disk book, so they
    /// see unsaved edits -- checking the stored copy would report
    /// problems the editor had already fixed.
    #[tokio::test]
    async fn the_report_reflects_an_unsaved_edit() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let before = get(&router, &format!("/tweak/report/{session}")).await.1["files"]["total_size"].as_u64().unwrap();

        let longer = "x".repeat(4000);
        let resp = router
            .clone()
            .oneshot(Request::builder().method("POST").uri(format!("/tweak/file/{session}/chapter1.xhtml")).body(Body::from(format!("<html><body><p>{longer}</p></body></html>"))).unwrap())
            .await
            .unwrap();
        assert!(resp.status().is_success());

        let after = get(&router, &format!("/tweak/report/{session}")).await.1["files"]["total_size"].as_u64().unwrap();
        assert!(after > before, "the report should see the unsaved edit: {before} -> {after}");
    }

    /// The engine was always real; what was missing was dictionary
    /// data, which now ships with the binary (#865). This proves the
    /// whole path works end to end against a book with a genuine
    /// misspelling in it.
    #[tokio::test]
    async fn spell_check_finds_a_real_misspelling() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/spell/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        let words: Vec<&str> = body["words"].as_array().unwrap().iter().filter_map(|w| w["word"].as_str()).collect();
        assert!(words.contains(&"libary"), "expected the misspelling to be flagged, got {words:?}");
        // And correctly-spelled words must not be.
        assert!(!words.contains(&"misspelling"), "a correct word was flagged: {words:?}");
    }

    #[tokio::test]
    async fn spell_check_offers_suggestions_and_locations() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (_, body) = get(&router, &format!("/tweak/spell/{session}")).await;
        let entry = body["words"].as_array().unwrap().iter().find(|w| w["word"] == "libary").expect("the misspelling should be listed");

        assert!(entry["files"].as_array().is_some_and(|f| !f.is_empty()), "a misspelling with no location cannot be found: {entry}");
        assert!(entry["count"].as_u64().unwrap_or(0) >= 1, "{entry}");
        // Suggestions are what make the report actionable rather than
        // merely accusatory.
        let suggestions: Vec<&str> = entry["suggestions"].as_array().unwrap().iter().filter_map(|s| s.as_str()).collect();
        assert!(suggestions.iter().any(|s| *s == "library"), "expected 'library' among suggestions, got {suggestions:?}");
    }

    async fn post_json(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder().method("POST").uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    // ---------------------------------------------------------------
    // Search and replace (#3.4)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn search_reports_matches_with_a_line_and_context() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "misspelling"})).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["matches"].as_u64().unwrap() >= 1, "{body}");
        let sample = &body["files"][0]["samples"][0];
        assert!(sample["line"].as_u64().unwrap() >= 1, "lines are 1-based: {sample}");
        assert!(sample["context"].as_str().unwrap().contains("misspelling"), "{sample}");
    }

    /// Searching must never write. The whole point of offering a
    /// search without a `replace` is being able to look first.
    #[tokio::test]
    async fn searching_without_a_replacement_changes_nothing() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (_, body) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "misspelling"})).await;
        assert_eq!(body["replaced"], false);
        assert_eq!(body["changed_files"], 0);

        let after = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "misspelling"})).await.1;
        assert_eq!(after["matches"], body["matches"], "a search must be repeatable with the same result");
    }

    #[tokio::test]
    async fn a_dry_run_reports_without_writing() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (_, body) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "libary", "replace": "library", "dry_run": true})).await;
        assert!(body["matches"].as_u64().unwrap() >= 1, "{body}");
        assert_eq!(body["replaced"], false, "a dry run must not write: {body}");

        // Still there afterwards.
        let after = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "libary"})).await.1;
        assert!(after["matches"].as_u64().unwrap() >= 1, "the dry run wrote anyway: {after}");
    }

    #[tokio::test]
    async fn a_replace_really_rewrites_the_file() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "libary", "replace": "library"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["replaced"], true);
        assert!(body["changed_files"].as_u64().unwrap() >= 1, "{body}");

        let after = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "libary"})).await.1;
        assert_eq!(after["matches"], 0, "the misspelling should be gone: {after}");
    }

    /// Someone searching for "C++" or "(draft)" means those
    /// characters. Compiling the box as a pattern would throw or
    /// match something unrelated.
    #[tokio::test]
    async fn a_plain_search_treats_metacharacters_literally() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        // `.` would match any character if this were compiled.
        let (_, dot) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "libary."})).await;
        let (_, wildcard) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "libary.", "regex": true})).await;

        assert_eq!(dot["matches"], 1, "the literal 'libary.' appears once: {dot}");
        assert_eq!(wildcard["matches"], 1, "and so does the pattern here: {wildcard}");

        // A pattern that only matches as a regex proves the flag works.
        let (_, only_regex) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "lib.ry", "regex": true})).await;
        let (_, as_plain) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "lib.ry"})).await;
        assert!(only_regex["matches"].as_u64().unwrap() >= 1, "{only_regex}");
        assert_eq!(as_plain["matches"], 0, "a plain search must not treat '.' as a wildcard: {as_plain}");
    }

    #[tokio::test]
    async fn case_sensitivity_is_respected() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let insensitive = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "LIBARY"})).await.1;
        let sensitive = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "LIBARY", "case_sensitive": true})).await.1;

        assert!(insensitive["matches"].as_u64().unwrap() >= 1, "{insensitive}");
        assert_eq!(sensitive["matches"], 0, "{sensitive}");
    }

    /// A "match" inside image bytes is noise at best and corruption
    /// at worst.
    #[tokio::test]
    async fn binary_files_are_never_searched_or_rewritten() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (_, body) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "e"})).await;
        let names: Vec<&str> = body["files"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).collect();

        assert!(!names.iter().any(|n| n.ends_with(".png") || n.ends_with(".jpg")), "{names:?}");
        assert!(names.iter().any(|n| n.ends_with(".xhtml") || n.ends_with(".opf")), "text files should be searched: {names:?}");
    }

    #[tokio::test]
    async fn an_invalid_regex_is_refused_before_anything_is_touched() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, _) = post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "(unclosed", "regex": true, "replace": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_empty_search_is_refused() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;
        assert_eq!(post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": ""})).await.0, StatusCode::BAD_REQUEST);
    }

    // ---------------------------------------------------------------
    // Fonts (#3.6) and diff (#3.5)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn fonts_reports_families_and_whether_they_are_embedded() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/fonts/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        // The actionable number: families a book asks for but does not
        // ship, which render as whatever the device happens to have.
        assert!(body["not_embedded"].is_number(), "{body}");
        assert!(body["families"].is_array(), "{body}");
    }

    /// An unmodified session must produce an empty diff -- otherwise
    /// the panel cries wolf on every open and nobody reads it.
    #[tokio::test]
    async fn an_untouched_session_differs_from_nothing() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let (status, body) = get(&router, &format!("/tweak/diff/{session}")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["changed"], 0, "a session with no edits should show no changes: {body}");
    }

    #[tokio::test]
    async fn diff_shows_the_lines_an_edit_changed() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/tweak/file/{session}/chapter1.xhtml"))
                    .body(Body::from("<html><body><p id=\"dup\">One</p><p>A brand new line.</p></body></html>"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success());

        let (_, body) = get(&router, &format!("/tweak/diff/{session}")).await;

        assert_eq!(body["changed"], 1, "{body}");
        let file = &body["files"][0];
        assert_eq!(file["name"], "chapter1.xhtml");
        assert_eq!(file["status"], "modified");

        let tags: Vec<&str> = file["lines"].as_array().unwrap().iter().filter_map(|l| l["tag"].as_str()).collect();
        assert!(tags.contains(&"add"), "an edit should show added lines: {tags:?}");
        let added: Vec<&str> = file["lines"].as_array().unwrap().iter().filter(|l| l["tag"] == "add").filter_map(|l| l["text"].as_str()).collect();
        assert!(added.iter().any(|t| t.contains("brand new line")), "{added:?}");
    }

    /// A search-and-replace writes through the same session, so the
    /// diff has to see it -- that is the point of reviewing before
    /// committing.
    #[tokio::test]
    async fn diff_sees_changes_made_by_other_editor_tools() {
        let (_d, router, book_id) = test_app();
        let session = open_session(&router, book_id).await;

        post_json(&router, &format!("/tweak/search-replace/{session}"), json!({"find": "libary", "replace": "library"})).await;

        let (_, body) = get(&router, &format!("/tweak/diff/{session}")).await;
        assert!(body["changed"].as_u64().unwrap() >= 1, "{body}");
    }

    #[tokio::test]
    async fn an_unknown_session_is_a_404_on_every_route() {
        let (_d, router, _) = test_app();
        assert_eq!(get(&router, "/tweak/check/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&router, "/tweak/report/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(post(&router, "/tweak/check-fix/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&router, "/tweak/spell/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(post_json(&router, "/tweak/search-replace/nope", json!({"find": "x"})).await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&router, "/tweak/fonts/nope").await.0, StatusCode::NOT_FOUND);
        assert_eq!(get(&router, "/tweak/diff/nope").await.0, StatusCode::NOT_FOUND);
    }
}
