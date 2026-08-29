//! Port of the NCX-navigation-generating slice of `old_src/src/calibre/
//! library/catalogs/epub_mobi_builder.py`'s `CatalogBuilder` class
//! (`generate_ncx_header`, `generate_ncx_section_header`,
//! `generate_ncx_subsection`, `generate_ncx_descriptions`,
//! `generate_ncx_by_series`, `generate_ncx_by_title`,
//! `generate_ncx_by_author`, `generate_ncx_by_date_added`,
//! `generate_ncx_by_genre`, `write_ncx`) -- cluster D of the
//! `epub_mobi_builder.py` port (issue #57; see `epub_mobi_builder.rs`'s
//! own module doc for clusters A-C, already complete). Split into its own
//! file purely for size (`epub_mobi_builder.rs` is already ~3900 lines);
//! this is still conceptually part of the same `CatalogBuilder` port.
//!
//! # Not ported: `generate_ncx_by_date_read`
//!
//! Same reasoning as `epub_mobi_builder::generate_html_by_date_read`
//! (see that function's own doc): it only ever does anything when
//! `bookmarked_books` is non-empty, and that's only ever populated by
//! `fetch_bookmarks`, already skipped in cluster A as device-specific.
//! Permanently dead in this port -- skipped, not stubbed.
//!
//! # Shared state: `NcxBuilder`
//!
//! Upstream threads two pieces of state through every `generate_ncx_*`
//! method as `self.ncx_root`/`self.play_order` -- an in-progress lxml
//! tree and a monotonically-increasing `playOrder` counter shared across
//! every call. Since this module has no `CatalogBuilder` struct yet
//! (that's cluster F, ported last), [`NcxBuilder`] is a small, local,
//! cluster-D-scoped struct bundling just that shared state (built via
//! [`calibre_ebooks::dom::Dom`] rather than lxml) -- narrower than the
//! full `CatalogBuilder`, not a preview of it.
//!
//! # Disclosed simplifications
//!
//! - **No pretty-printing on write.** Upstream's `write_ncx` calls
//!   `pretty_xml_tree` before serializing; [`calibre_ebooks::dom::Dom`]
//!   has no equivalent indentation pass. Cosmetic only -- NCX consumers
//!   don't care about whitespace.
//! - **`generate_ncx_by_date_added`'s multi-bucket day-range logic is
//!   fixed, not preserved**, for the same reason and in the same way as
//!   `epub_mobi_builder::generate_html_by_date_added`'s identical bug
//!   (re-scans the whole list every iteration, seeds the next bucket
//!   with a duplicated leftover entry) -- unreachable with the shipped
//!   `DATE_RANGE=[30]` default, so this port implements the documented,
//!   correct, non-overlapping-bucket intent instead.
//! - **`generate_ncx_by_genre`'s `friendly_tag` lookup reuses
//!   [`super::epub_mobi_builder::get_friendly_genre_tag`]** rather than
//!   transliterating upstream's own inline linear search (a `for
//!   friendly_tag in dict: if dict[friendly_tag] == tag: break` loop
//!   relying on Python's loop-variable leaking into the enclosing scope
//!   after the `break` -- functionally identical to the already-ported
//!   helper, just written out by hand instead of calling it).

use serde_json::Value;

use calibre_ebooks::dom::{Dom, NodeId};

use super::epub_mobi_builder::{
    establish_equivalencies, format_ncx_text, generate_short_description, generate_sort_title, generate_unicode_name,
    get_friendly_genre_tag, letter_or_symbol_str, GenrePage, ShortDescriptionDest, SYMBOLS,
};

fn book_str<'a>(book: &'a Value, field: &str) -> &'a str {
    book.get(field).and_then(|v| v.as_str()).unwrap_or("")
}

fn set_attr(dom: &mut Dom, id: NodeId, name: &str, value: impl Into<String>) {
    dom.node_mut(id).attrs.insert(name.to_string(), value.into());
}

fn append_text(dom: &mut Dom, parent: NodeId, text: &str) {
    let t = dom.new_text(text);
    dom.append_child(parent, t);
}

/// Bundles upstream's `self.ncx_root`/`self.play_order` -- see this
/// module's doc for why this is a narrow, cluster-D-scoped struct rather
/// than a preview of the full (not-yet-ported) `CatalogBuilder`.
pub struct NcxBuilder {
    pub dom: Dom,
    /// The element every top-level section [`NcxBuilder::section_header`]
    /// call appends to: the plain `<navMap>` normally, or (for Kindle/MOBI)
    /// the single top-level periodical `<navPoint>` `generate_ncx_header`
    /// builds.
    section_container: NodeId,
    play_order: i64,
    generate_for_kindle_mobi: bool,
}

impl NcxBuilder {
    /// Port of `generate_ncx_header`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        generate_for_kindle_mobi: bool,
        catalog_title: &str,
        generate_authors: bool,
        generate_titles: bool,
        generate_series: bool,
        generate_genres: bool,
        generate_recently_added: bool,
        generate_descriptions: bool,
        first_genre_file: Option<&str>,
        first_description_book_id: Option<i64>,
    ) -> NcxBuilder {
        let mut dom = Dom::empty();
        let root = dom.root;
        let ncx = dom.new_element("ncx");
        set_attr(&mut dom, ncx, "xmlns", "http://www.daisy.org/z3986/2005/ncx/");
        set_attr(&mut dom, ncx, "xmlns:calibre", "http://calibre.kovidgoyal.net/2009/metadata");
        set_attr(&mut dom, ncx, "version", "2005-1");
        set_attr(&mut dom, ncx, "xml:lang", "en");
        dom.append_child(root, ncx);

        let nav_map = dom.new_element("navMap");
        dom.append_child(ncx, nav_map);

        let mut play_order = 1i64;
        let mut section_container = nav_map;

        if generate_for_kindle_mobi {
            let nav_point = dom.new_element("navPoint");
            set_attr(&mut dom, nav_point, "class", "periodical");
            set_attr(&mut dom, nav_point, "id", "title");
            set_attr(&mut dom, nav_point, "playOrder", play_order.to_string());
            play_order += 1;
            dom.append_child(nav_map, nav_point);

            let meta_img = dom.new_element("calibre:meta-img");
            set_attr(&mut dom, meta_img, "id", "mastheadImage");
            set_attr(&mut dom, meta_img, "src", "images/mastheadImage.gif");
            dom.append_child(nav_point, meta_img);

            let nav_label = dom.new_element("navLabel");
            let text_el = dom.new_element("text");
            append_text(&mut dom, text_el, catalog_title);
            dom.append_child(nav_label, text_el);
            dom.append_child(nav_point, nav_label);

            let content_src = if generate_authors {
                Some("content/ByAlphaAuthor.html".to_string())
            } else if generate_titles {
                Some("content/ByAlphaTitle.html".to_string())
            } else if generate_series {
                Some("content/BySeries.html".to_string())
            } else if generate_genres {
                first_genre_file.map(|s| s.to_string())
            } else if generate_recently_added {
                Some("content/ByDateAdded.html".to_string())
            } else if generate_descriptions {
                first_description_book_id.map(|id| format!("content/book_{id}.html"))
            } else {
                None
            };
            if let Some(src) = content_src {
                let content = dom.new_element("content");
                set_attr(&mut dom, content, "src", src);
                dom.append_child(nav_point, content);
            }

            section_container = nav_point;
        }

        NcxBuilder { dom, section_container, play_order, generate_for_kindle_mobi }
    }

    /// Port of `generate_ncx_section_header`.
    pub fn section_header(&mut self, section_id: &str, section_header: &str, content_src: &str) -> NodeId {
        let nav_point = self.dom.new_element("navPoint");
        set_attr(&mut self.dom, nav_point, "id", section_id);
        set_attr(&mut self.dom, nav_point, "playOrder", self.play_order.to_string());
        if self.generate_for_kindle_mobi {
            set_attr(&mut self.dom, nav_point, "class", "section");
        }
        self.play_order += 1;
        self.dom.append_child(self.section_container, nav_point);

        let nav_label = self.dom.new_element("navLabel");
        let text_el = self.dom.new_element("text");
        append_text(&mut self.dom, text_el, section_header);
        self.dom.append_child(nav_label, text_el);
        self.dom.append_child(nav_point, nav_label);

        let content = self.dom.new_element("content");
        set_attr(&mut self.dom, content, "src", content_src);
        self.dom.append_child(nav_point, content);

        nav_point
    }

    /// Port of `generate_ncx_subsection`.
    pub fn subsection(&mut self, parent: NodeId, section_id: &str, section_text: &str, content_src: &str, cm_tags: &[(&str, &str)]) {
        let nav_point = self.dom.new_element("navPoint");
        set_attr(&mut self.dom, nav_point, "id", section_id);
        set_attr(&mut self.dom, nav_point, "playOrder", self.play_order.to_string());
        if self.generate_for_kindle_mobi {
            set_attr(&mut self.dom, nav_point, "class", "article");
        }
        self.play_order += 1;
        self.dom.append_child(parent, nav_point);

        let nav_label = self.dom.new_element("navLabel");
        let text_el = self.dom.new_element("text");
        append_text(&mut self.dom, text_el, section_text);
        self.dom.append_child(nav_label, text_el);
        self.dom.append_child(nav_point, nav_label);

        let content = self.dom.new_element("content");
        set_attr(&mut self.dom, content, "src", content_src);
        self.dom.append_child(nav_point, content);

        if self.generate_for_kindle_mobi {
            for (name, text) in cm_tags {
                let meta = self.dom.new_element("calibre:meta");
                set_attr(&mut self.dom, meta, "name", *name);
                append_text(&mut self.dom, meta, text);
                self.dom.append_child(nav_point, meta);
            }
        }
    }

    /// Port of `write_ncx`: the finished `<basename>.ncx` document
    /// string. Writing it to `catalog_path` is the caller's job (cluster
    /// F), matching this whole port's "pure function/builder, caller
    /// does I/O" convention.
    pub fn write(&self) -> String {
        format!("<?xml version='1.0' encoding='utf-8'?>\n{}", self.dom.serialize(self.dom.root))
    }
}

/// Port of `generate_ncx_descriptions`. `books_by_description` is
/// `self.books_by_description` (`fetch_books_by_author`'s own output
/// when `generate_descriptions` is set, or `books_by_title` when
/// `sort_descriptions_by_author` is false -- caller's choice, matching
/// upstream's own `self.opts.sort_descriptions_by_author` branch).
pub fn generate_ncx_descriptions(
    ncx: &mut NcxBuilder,
    toc_title: &str,
    books_by_description: &[Value],
    generate_for_kindle_mobi: bool,
    author_clip: usize,
    description_clip: usize,
) {
    if books_by_description.is_empty() {
        return;
    }
    let section_header = if generate_for_kindle_mobi {
        toc_title.to_string()
    } else {
        format!("{toc_title} [{}]", books_by_description.len())
    };
    let first_id = books_by_description[0].get("id").and_then(|v| v.as_i64()).unwrap_or_default();
    let nav_point = ncx.section_header("bydescription-ID", &section_header, &format!("content/book_{first_id}.html"));

    for book in books_by_description {
        let book_id = book.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
        let sec_id = format!("book{book_id}ID");
        let title = book_str(book, "title");
        let author = book_str(book, "author");

        let sec_text = match book.get("series").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(series) => {
                let series_index = book.get("series_index").map(|v| v.to_string()).unwrap_or_default();
                let series_index = series_index.strip_suffix(".0").unwrap_or(&series_index);
                if generate_for_kindle_mobi {
                    format_ncx_text(Some(&format!("{title} ({series} [{series_index}])")), Some(ShortDescriptionDest::Title), author_clip, description_clip)
                        .unwrap_or_default()
                } else {
                    format_ncx_text(
                        Some(&format!("{title} ({series} [{series_index}]) \u{b7} {author} ")),
                        Some(ShortDescriptionDest::Title),
                        author_clip,
                        description_clip,
                    )
                    .unwrap_or_default()
                }
            }
            None if generate_for_kindle_mobi => {
                format_ncx_text(Some(title), Some(ShortDescriptionDest::Title), author_clip, description_clip).unwrap_or_default()
            }
            None => format_ncx_text(Some(&format!("{title} \u{b7} {author}")), Some(ShortDescriptionDest::Title), author_clip, description_clip)
                .unwrap_or_default(),
        };

        let content_src = format!("content/book_{book_id}.html#book{book_id}");

        let mut nav_str = match book.get("date").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(date) => {
                let year = date.split_whitespace().nth(1).unwrap_or("");
                format!(
                    "{} | {}",
                    format_ncx_text(Some(author), Some(ShortDescriptionDest::Author), author_clip, description_clip).unwrap_or_default(),
                    year
                )
            }
            None => format_ncx_text(Some(author), Some(ShortDescriptionDest::Author), author_clip, description_clip).unwrap_or_default(),
        };
        if let Some(tags) = book.get("tags").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
            let mut sorted_tags: Vec<&str> = tags.iter().filter_map(|v| v.as_str()).collect();
            sorted_tags.sort();
            nav_str = format_ncx_text(
                Some(&format!("{nav_str} | {}", sorted_tags.join(" \u{b7} "))),
                Some(ShortDescriptionDest::Author),
                author_clip,
                description_clip,
            )
            .unwrap_or_default();
        }

        let mut cm_tags: Vec<(&str, String)> = vec![("author", nav_str)];
        if let Some(short) = book.get("short_description").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            let d = format_ncx_text(Some(short), Some(ShortDescriptionDest::Description), author_clip, description_clip).unwrap_or_default();
            cm_tags.push(("description", d));
        }
        let cm_tags_ref: Vec<(&str, &str)> = cm_tags.iter().map(|(k, v)| (*k, v.as_str())).collect();
        ncx.subsection(nav_point, &sec_id, &sec_text, &content_src, &cm_tags_ref);
    }
}

/// Shared shape of upstream's `_add_to_series_by_letter`/
/// `_add_to_books_by_letter`/`_add_to_author_list` inner closures +
/// their enclosing loops: bucket `items` by leading-letter equivalence
/// class, accumulating up to `preview_limit` distinct-in-a-row
/// "identity" values (deduped consecutively, not globally -- matching
/// upstream's own `!= current_identity` check) into a `" \u{2022} "`
/// -joined, [`format_ncx_text`]-massaged preview string per bucket.
/// `self.authors` (already deduped) passing `identity_of == display_of`
/// makes the dedup check a harmless no-op for that caller, matching
/// upstream not having a dedup check there at all.
fn bucket_preview_by_letter<T>(
    items: &[T],
    sort_keys: &[String],
    identity_of: impl Fn(&T) -> String,
    display_of: impl Fn(&T) -> String,
    preview_limit: usize,
    description_clip: usize,
) -> Vec<(String, String)> {
    if items.is_empty() {
        return Vec::new();
    }
    let sort_equivalents = establish_equivalencies(sort_keys);
    let mut buckets: Vec<(String, String)> = Vec::new();
    let mut current_letter = letter_or_symbol_str(&sort_equivalents[0]);
    let mut current_identity = String::new();
    let mut current_list: Vec<String> = Vec::new();

    fn flush(list: &mut Vec<String>, buckets: &mut Vec<(String, String)>, letter: &str, description_clip: usize) {
        if list.is_empty() {
            return;
        }
        let joined = list.join(" \u{2022} ");
        let text = format_ncx_text(Some(&joined), Some(ShortDescriptionDest::Description), 0, description_clip).unwrap_or_default();
        buckets.push((letter.to_string(), text));
        list.clear();
    }

    for (idx, item) in items.iter().enumerate() {
        let letter = letter_or_symbol_str(&sort_equivalents[idx]);
        if letter != current_letter {
            flush(&mut current_list, &mut buckets, &current_letter, description_clip);
            current_letter = letter;
            current_identity = identity_of(item);
            current_list = vec![display_of(item)];
        } else if current_list.len() < preview_limit && identity_of(item) != current_identity {
            current_identity = identity_of(item);
            current_list.push(display_of(item));
        }
    }
    flush(&mut current_list, &mut buckets, &current_letter, description_clip);
    buckets
}

/// Port of `generate_ncx_by_series`.
pub fn generate_ncx_by_series(
    ncx: &mut NcxBuilder,
    toc_title: &str,
    books_by_series: &[Value],
    all_series_count: usize,
    generate_for_kindle_mobi: bool,
    description_clip: usize,
) {
    if books_by_series.is_empty() {
        return;
    }
    let section_header = if generate_for_kindle_mobi { toc_title.to_string() } else { format!("{toc_title} [{all_series_count}]") };
    let nav_point = ncx.section_header("byseries-ID", &section_header, "content/BySeries.html#section_start");

    let sort_keys: Vec<String> = books_by_series.iter().map(|b| generate_sort_title(book_str(b, "series"))).collect();
    let buckets = bucket_preview_by_letter(
        books_by_series,
        &sort_keys,
        |b| book_str(b, "series").to_string(),
        |b| book_str(b, "series").to_string(),
        description_clip,
        description_clip,
    );

    for (letter, text) in &buckets {
        let sec_id = format!("{}Series-ID", letter.to_uppercase());
        let sec_text = if letter.chars().count() > 1 {
            format!("Series beginning with {letter}")
        } else {
            format!("Series beginning with '{letter}'")
        };
        let content_src = if letter == SYMBOLS {
            format!("content/BySeries.html#{SYMBOLS}_series")
        } else {
            format!("content/BySeries.html#{}_series", generate_unicode_name(letter))
        };
        ncx.subsection(nav_point, &sec_id, &sec_text, &content_src, &[("description", text)]);
    }
}

/// Port of `generate_ncx_by_title`. Same "`use_series_prefix_in_titles_
/// section` is always false" simplification as
/// `epub_mobi_builder::generate_html_by_title` -- `books_by_title` is
/// used directly rather than reproducing the always-taken, content-
/// equivalent `books_by_title_no_series_prefix` re-derivation.
pub fn generate_ncx_by_title(
    ncx: &mut NcxBuilder,
    toc_title: &str,
    books_by_title: &[Value],
    generate_for_kindle_mobi: bool,
    description_clip: usize,
) {
    if books_by_title.is_empty() {
        return;
    }
    let section_header = if generate_for_kindle_mobi { toc_title.to_string() } else { format!("{toc_title} [{}]", books_by_title.len()) };
    let nav_point = ncx.section_header("byalphatitle-ID", &section_header, "content/ByAlphaTitle.html#section_start");

    let sort_keys: Vec<String> = books_by_title.iter().map(|b| book_str(b, "title_sort").to_string()).collect();
    let buckets = bucket_preview_by_letter(
        books_by_title,
        &sort_keys,
        |b| book_str(b, "title").to_string(),
        |b| book_str(b, "title").to_string(),
        description_clip,
        description_clip,
    );

    for (letter, text) in &buckets {
        let sec_id = format!("{}Titles-ID", letter.to_uppercase());
        let sec_text = if letter.chars().count() > 1 {
            format!("Titles beginning with {letter}")
        } else {
            format!("Titles beginning with '{letter}'")
        };
        let content_src = if letter == SYMBOLS {
            format!("content/ByAlphaTitle.html#{SYMBOLS}_titles")
        } else {
            format!("content/ByAlphaTitle.html#{}_titles", generate_unicode_name(letter))
        };
        ncx.subsection(nav_point, &sec_id, &sec_text, &content_src, &[("description", text)]);
    }
}

/// Port of `generate_ncx_by_author`. `authors` is `fetch_books_by_author`'s
/// own already-deduplicated `(friendly, sort, count)` list.
pub fn generate_ncx_by_author(
    ncx: &mut NcxBuilder,
    toc_title: &str,
    authors: &[(String, String, usize)],
    individual_authors_count: usize,
    generate_for_kindle_mobi: bool,
    description_clip: usize,
) {
    if authors.is_empty() {
        return;
    }
    let file_id = toc_title.to_lowercase().replace(' ', "");
    let section_header = if generate_for_kindle_mobi { toc_title.to_string() } else { format!("{toc_title} [{individual_authors_count}]") };
    let nav_point = ncx.section_header(&format!("{file_id}-ID"), &section_header, "content/ByAlphaAuthor.html#section_start");

    let sort_keys: Vec<String> = authors.iter().map(|(_, sort, _)| sort.clone()).collect();
    let buckets = bucket_preview_by_letter(authors, &sort_keys, |a| a.0.clone(), |a| a.0.clone(), description_clip, description_clip);

    for (letter, text) in &buckets {
        let sec_id = format!("{letter}authors-ID");
        let sec_text = if letter == SYMBOLS { format!("Authors beginning with {letter}") } else { format!("Authors beginning with '{letter}'") };
        let content_src = if letter == SYMBOLS {
            format!("content/ByAlphaAuthor.html#{SYMBOLS}_authors")
        } else {
            format!("content/ByAlphaAuthor.html#{}_authors", generate_unicode_name(letter))
        };
        ncx.subsection(nav_point, &sec_id, &sec_text, &content_src, &[("description", text)]);
    }
}

/// Port of `generate_ncx_by_date_added`. `now`/`date_ranges_days`
/// mirror `epub_mobi_builder::generate_html_by_date_added`'s own
/// parameters, including the same "implement the documented, correct,
/// non-overlapping-bucket intent instead of upstream's broken and
/// unreachable-with-the-shipped-default multi-bucket logic" fix -- see
/// that function's doc and this module's own doc for the full
/// reasoning, identical here.
pub fn generate_ncx_by_date_added(
    ncx: &mut NcxBuilder,
    toc_title: &str,
    books_by_date_range: &[Value],
    date_ranges_days: &[i64],
    now: chrono::DateTime<chrono::Utc>,
    description_clip: usize,
) {
    use chrono::Datelike;

    if books_by_date_range.is_empty() {
        return;
    }
    let file_id = toc_title.to_lowercase().replace(' ', "");
    let nav_point = ncx.section_header(&format!("{file_id}-ID"), toc_title, "content/ByDateAdded.html#section_start");

    let mut by_date: Vec<(&Value, chrono::DateTime<chrono::Utc>)> = books_by_date_range
        .iter()
        .filter_map(|b| b.get("timestamp").and_then(|v| v.as_str()).and_then(|s| calibre_utils::date::parse_date(s, true)).map(|ts| (b, ts)))
        .collect();
    by_date.sort_by(|a, b| b.1.cmp(&a.1));

    let mut lower_bound = 0i64;
    for (i, &limit) in date_ranges_days.iter().enumerate() {
        let label = if i == 0 { format!("Last {limit} days") } else { format!("{} to {limit} days ago", date_ranges_days[i - 1]) };
        let bucket: Vec<&str> = by_date
            .iter()
            .filter(|(_, ts)| {
                let days = (now - *ts).num_days();
                days > lower_bound && days <= limit
            })
            .map(|(b, _)| book_str(b, "title"))
            .collect();
        lower_bound = limit;
        if bucket.is_empty() {
            continue;
        }
        let count = bucket.len();
        let text = format_ncx_text(Some(&bucket.join(" \u{2022} ")), Some(ShortDescriptionDest::Description), 0, description_clip).unwrap_or_default();
        let sec_id = format!("{}-ID", label.replace(' ', ""));
        let content_src = format!("content/ByDateAdded.html#bda_{}", label.replace(' ', ""));
        let nav_str = if count > 1 { format!("{count} titles") } else { format!("{count} title") };
        ncx.subsection(nav_point, &sec_id, &label, &content_src, &[("description", &text), ("author", &nav_str)]);
    }

    let mut current_ym: Option<(i32, u32)> = None;
    let mut current_titles: Vec<&str> = Vec::new();
    let mut months: Vec<(i32, u32, Vec<&str>)> = Vec::new();
    for (book, ts) in &by_date {
        let ym = (ts.year(), ts.month());
        if Some(ym) != current_ym {
            if let Some((y, m)) = current_ym {
                months.push((y, m, std::mem::take(&mut current_titles)));
            }
            current_ym = Some(ym);
        }
        current_titles.push(book_str(book, "title"));
    }
    if let Some((y, m)) = current_ym {
        months.push((y, m, current_titles));
    }

    for (year, month, titles) in &months {
        let count = titles.len();
        let text = format_ncx_text(Some(&titles.join(" \u{2022} ")), Some(ShortDescriptionDest::Description), 0, description_clip).unwrap_or_default();
        let month_name = by_date.iter().find(|(_, t)| (t.year(), t.month()) == (*year, *month)).unwrap().1.format("%B %Y").to_string();
        let sec_id = format!("bda_{year}-{month}-ID");
        let content_src = format!("content/ByDateAdded.html#bda_{year}-{month}");
        let nav_str = if count > 1 { format!("{count} titles") } else { format!("{count} title") };
        ncx.subsection(nav_point, &sec_id, &month_name, &content_src, &[("description", &text), ("author", &nav_str)]);
    }
}

/// Port of `generate_ncx_by_genre`. `genres` is
/// `generate_html_by_genres`'s own [`GenrePage`] list;
/// `genre_tags_dict` is `filter_genre_tags`'s output, needed to recover
/// each genre's friendly display name.
pub fn generate_ncx_by_genre(
    ncx: &mut NcxBuilder,
    toc_title: &str,
    genres: &[GenrePage],
    genre_tags_dict: &indexmap::IndexMap<String, String>,
    generate_for_kindle_mobi: bool,
    description_clip: usize,
) {
    if genres.is_empty() {
        return;
    }
    let file_id = toc_title.to_lowercase().replace(' ', "");
    let section_header = if generate_for_kindle_mobi { toc_title.to_string() } else { format!("{toc_title} [{}]", genres.len()) };
    let nav_point =
        ncx.section_header(&format!("{file_id}-ID"), &section_header, &format!("content/Genre_{}.html#section_start", genres[0].tag));

    for genre in genres {
        let sec_id = format!("genre-{}-ID", genre.tag);
        let friendly_tag = get_friendly_genre_tag(genre_tags_dict, &genre.tag).unwrap_or(genre.tag.as_str());
        let sec_text = format_ncx_text(Some(friendly_tag), Some(ShortDescriptionDest::Description), 0, description_clip).unwrap_or_default();
        let content_src = format!("content/Genre_{0}.html#Genre_{0}", genre.tag);

        let author_range = if genre.titles_spanned.len() > 1 {
            format!("{} - {}", genre.titles_spanned[0].0, genre.titles_spanned[1].0)
        } else {
            genre.titles_spanned[0].0.clone()
        };

        let mut titles: Vec<String> = genre.books.iter().map(|b| book_str(b, "title").to_string()).collect();
        titles.sort_by(|a, b| generate_sort_title(a).cmp(&generate_sort_title(b)));
        let titles_list = generate_short_description(Some(&titles.join(" \u{2022} ")), ShortDescriptionDest::Description, 0, description_clip)
            .unwrap_or_default();
        let description = format_ncx_text(Some(&titles_list), Some(ShortDescriptionDest::Description), 0, description_clip).unwrap_or_default();

        ncx.subsection(nav_point, &sec_id, &sec_text, &content_src, &[("author", &author_range), ("description", &description)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ncx_book(fields: &[(&str, Value)]) -> Value {
        let mut defaults: Vec<(&str, Value)> = vec![
            ("id", Value::from(1)),
            ("title", Value::from("Book One")),
            ("title_sort", Value::from("Book One")),
            ("author", Value::from("Alice")),
            ("author_sort", Value::from("Alice")),
            ("series", Value::Null),
            ("series_index", Value::from(0.0)),
            ("date", Value::Null),
            ("timestamp", Value::Null),
            ("tags", Value::from(Vec::<String>::new())),
            ("short_description", Value::Null),
        ];
        for (k, v) in fields {
            if let Some(entry) = defaults.iter_mut().find(|(dk, _)| dk == k) {
                entry.1 = v.clone();
            } else {
                defaults.push((k, v.clone()));
            }
        }
        Value::Object(defaults.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn new_epub_ncx() -> NcxBuilder {
        NcxBuilder::new(false, "My Catalog", true, true, true, true, true, true, None, None)
    }

    #[test]
    fn header_builds_a_plain_navmap_for_epub() {
        let ncx = new_epub_ncx();
        let xml = ncx.write();
        assert!(xml.starts_with("<?xml"), "{xml}");
        assert!(xml.contains("<navMap>"), "{xml}");
        assert!(!xml.contains("periodical"), "{xml}");
    }

    #[test]
    fn header_builds_a_periodical_wrapper_for_kindle_mobi() {
        let ncx = NcxBuilder::new(true, "My Catalog", true, false, false, false, false, false, None, None);
        let xml = ncx.write();
        assert!(xml.contains("class=\"periodical\""), "{xml}");
        assert!(xml.contains("content/ByAlphaAuthor.html"), "{xml}");
    }

    #[test]
    fn section_header_and_subsection_increment_play_order() {
        let mut ncx = new_epub_ncx();
        let section = ncx.section_header("sec-ID", "Section", "content/Section.html");
        ncx.subsection(section, "sub-ID", "Sub", "content/Section.html#sub", &[]);
        let xml = ncx.write();
        assert!(xml.contains("playOrder=\"1\""), "{xml}");
        assert!(xml.contains("playOrder=\"2\""), "{xml}");
    }

    #[test]
    fn subsection_adds_meta_tags_only_for_kindle_mobi() {
        let mut epub_ncx = new_epub_ncx();
        let sec = epub_ncx.section_header("sec-ID", "Section", "content/Section.html");
        epub_ncx.subsection(sec, "sub-ID", "Sub", "content/Section.html#sub", &[("author", "Alice")]);
        assert!(!epub_ncx.write().contains("calibre:meta"));

        let mut mobi_ncx = NcxBuilder::new(true, "Cat", false, false, false, false, false, false, None, None);
        let sec2 = mobi_ncx.section_header("sec-ID", "Section", "content/Section.html");
        mobi_ncx.subsection(sec2, "sub-ID", "Sub", "content/Section.html#sub", &[("author", "Alice")]);
        assert!(mobi_ncx.write().contains("calibre:meta"));
    }

    #[test]
    fn descriptions_produces_no_op_for_an_empty_list() {
        let mut ncx = new_epub_ncx();
        generate_ncx_descriptions(&mut ncx, "Descriptions", &[], false, 100, 100);
        assert!(!ncx.write().contains("bydescription-ID"));
    }

    #[test]
    fn descriptions_includes_author_for_non_kindle() {
        let mut ncx = new_epub_ncx();
        let books = vec![ncx_book(&[])];
        generate_ncx_descriptions(&mut ncx, "Descriptions", &books, false, 100, 100);
        let xml = ncx.write();
        assert!(xml.contains("Book One \u{b7} Alice"), "{xml}");
    }

    #[test]
    fn descriptions_omits_author_for_kindle_mobi() {
        let mut ncx = NcxBuilder::new(true, "Cat", false, false, false, false, false, true, None, Some(1));
        let books = vec![ncx_book(&[])];
        generate_ncx_descriptions(&mut ncx, "Descriptions", &books, true, 100, 100);
        let xml = ncx.write();
        assert!(!xml.contains("Book One \u{b7} Alice"), "{xml}");
    }

    #[test]
    fn descriptions_includes_series_when_present() {
        let mut ncx = new_epub_ncx();
        let books = vec![ncx_book(&[("series", Value::from("Foundation")), ("series_index", Value::from(2.0))])];
        generate_ncx_descriptions(&mut ncx, "Descriptions", &books, false, 100, 100);
        let xml = ncx.write();
        assert!(xml.contains("Book One (Foundation [2])"), "{xml}");
    }

    #[test]
    fn descriptions_includes_short_description_when_present() {
        // cm_tags (short_description/author nav string) are only ever
        // added as <calibre:meta> for Kindle/MOBI -- matching upstream's
        // own `if self.generate_for_kindle_mobi:` gate around that loop
        // in generate_ncx_subsection -- so this needs a kindle_mobi
        // builder to observe, unlike sec_text (always present).
        let mut ncx = NcxBuilder::new(true, "Cat", false, false, false, false, false, true, None, Some(1));
        let books = vec![ncx_book(&[("short_description", Value::from("A great book"))])];
        generate_ncx_descriptions(&mut ncx, "Descriptions", &books, true, 100, 100);
        assert!(ncx.write().contains("A great book"));
    }

    // --- generate_ncx_by_series / by_title / by_author ---

    #[test]
    fn by_series_is_a_no_op_for_an_empty_list() {
        let mut ncx = new_epub_ncx();
        generate_ncx_by_series(&mut ncx, "Series", &[], 0, false, 100);
        assert!(!ncx.write().contains("byseries-ID"));
    }

    #[test]
    fn by_series_creates_one_subsection_per_letter() {
        let mut ncx = new_epub_ncx();
        let books = vec![
            ncx_book(&[("series", Value::from("Apple Series"))]),
            ncx_book(&[("series", Value::from("Zebra Series"))]),
        ];
        generate_ncx_by_series(&mut ncx, "Series", &books, 2, false, 100);
        let xml = ncx.write();
        assert!(xml.contains("ASeries-ID"), "{xml}");
        assert!(xml.contains("ZSeries-ID"), "{xml}");
        // The anchor is keyed by the LETTER bucket, not the individual
        // series name -- matches upstream's own
        // `generate_unicode_name(title_letters[i])`, not
        // `generate_series_anchor(series)`.
        assert!(xml.contains("content/BySeries.html#LATIN_CAPITAL_LETTER_A_series"), "{xml}");
    }

    #[test]
    fn by_title_creates_one_subsection_per_letter() {
        let mut ncx = new_epub_ncx();
        let books = vec![
            ncx_book(&[("title", Value::from("Apple")), ("title_sort", Value::from("Apple"))]),
            ncx_book(&[("title", Value::from("Zebra")), ("title_sort", Value::from("Zebra"))]),
        ];
        generate_ncx_by_title(&mut ncx, "Titles", &books, false, 100);
        let xml = ncx.write();
        assert!(xml.contains("ATitles-ID"), "{xml}");
        assert!(xml.contains("ZTitles-ID"), "{xml}");
    }

    #[test]
    fn by_author_creates_one_subsection_per_letter() {
        let mut ncx = new_epub_ncx();
        let authors = vec![("Alice".to_string(), "Alice".to_string(), 1usize), ("Zeb".to_string(), "Zeb".to_string(), 1usize)];
        generate_ncx_by_author(&mut ncx, "Authors", &authors, 2, false, 100);
        let xml = ncx.write();
        assert!(xml.contains("Aauthors-ID"), "{xml}");
        assert!(xml.contains("Zauthors-ID"), "{xml}");
        // The author names themselves only appear in the `description`
        // cm_tag, which -- like generate_ncx_descriptions's own
        // author/description tags -- is only emitted for Kindle/MOBI;
        // sec_text here is just "Authors beginning with '<letter>'".
        assert!(xml.contains("Authors beginning with 'A'"), "{xml}");
    }

    #[test]
    fn by_author_includes_the_author_name_preview_for_kindle_mobi() {
        let mut ncx = NcxBuilder::new(true, "Cat", true, false, false, false, false, false, None, None);
        let authors = vec![("Alice".to_string(), "Alice".to_string(), 1usize)];
        generate_ncx_by_author(&mut ncx, "Authors", &authors, 1, true, 100);
        assert!(ncx.write().contains("Alice"));
    }

    // --- generate_ncx_by_date_added ---

    fn a_now() -> chrono::DateTime<chrono::Utc> {
        calibre_utils::date::parse_date("2024-03-15T12:00:00Z", true).unwrap()
    }

    #[test]
    fn by_date_added_is_a_no_op_for_an_empty_list() {
        let mut ncx = new_epub_ncx();
        generate_ncx_by_date_added(&mut ncx, "Recently Added", &[], &[30], a_now(), 100);
        assert!(!ncx.write().contains("recentlyadded-ID"));
    }

    #[test]
    fn by_date_added_creates_a_day_range_and_month_bucket() {
        let mut ncx = new_epub_ncx();
        let books = vec![ncx_book(&[("timestamp", Value::from("2024-03-14T00:00:00Z"))])];
        generate_ncx_by_date_added(&mut ncx, "Recently Added", &books, &[30], a_now(), 100);
        let xml = ncx.write();
        assert!(xml.contains("Last30days-ID"), "{xml}");
        assert!(xml.contains("bda_2024-3-ID"), "{xml}");
    }

    #[test]
    fn by_date_added_includes_the_title_preview_for_kindle_mobi() {
        let mut ncx = NcxBuilder::new(true, "Cat", false, false, false, false, true, false, None, None);
        let books = vec![ncx_book(&[("timestamp", Value::from("2024-03-14T00:00:00Z"))])];
        generate_ncx_by_date_added(&mut ncx, "Recently Added", &books, &[30], a_now(), 100);
        assert!(ncx.write().contains("Book One"));
    }

    // --- generate_ncx_by_genre ---

    #[test]
    fn by_genre_is_a_no_op_for_an_empty_list() {
        let mut ncx = new_epub_ncx();
        let dict = indexmap::IndexMap::new();
        generate_ncx_by_genre(&mut ncx, "Genres", &[], &dict, false, 100);
        assert!(!ncx.write().contains("genre-"));
    }

    #[test]
    fn by_genre_creates_one_subsection_per_genre_using_the_friendly_name() {
        let mut ncx = new_epub_ncx();
        let mut dict = indexmap::IndexMap::new();
        dict.insert("Sci-Fi".to_string(), "scifi".to_string());
        let genres = vec![GenrePage {
            tag: "scifi".to_string(),
            file: "content/Genre_scifi.html".to_string(),
            authors: vec![("Alice".to_string(), "Alice".to_string(), 1)],
            books: vec![ncx_book(&[])],
            titles_spanned: vec![("Alice".to_string(), "Book One".to_string())],
            html: String::new(),
        }];
        generate_ncx_by_genre(&mut ncx, "Genres", &genres, &dict, false, 100);
        let xml = ncx.write();
        assert!(xml.contains("genre-scifi-ID"), "{xml}");
        assert!(xml.contains("content/Genre_scifi.html#Genre_scifi"), "{xml}");
        assert!(xml.contains("Sci-Fi"), "{xml}");
    }
}
