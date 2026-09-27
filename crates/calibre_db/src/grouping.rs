//! Deciding which discovered files are the same book (issue #892, part
//! of #889).
//!
//! # Filenames are not evidence
//!
//! calibre groups by filename stem, and so does this crate's own
//! `find_books_in_directory`: `Dune.epub` and `Dune.pdf` become one book
//! with two formats. That works for a library the app itself named, and
//! it is wrong for a folder somebody else filled. Two files called
//! `scan0001` in different folders have nothing to do with each other,
//! and a `.txt` sitting beside a `.pdf` is far more often OCR output
//! than an alternative edition of the same book.
//!
//! So the stem plays no part here. Files are the same book only when
//! something actually says so:
//!
//! | Evidence | Conclusion |
//! | --- | --- |
//! | Byte-identical content | the same file twice: a duplicate |
//! | Specific matching metadata, **different** formats | one book, several formats |
//! | Specific matching metadata, **same** format | probably one book twice: a duplicate |
//! | Anything else | separate books |
//!
//! # Why a hash match is a duplicate rather than a format
//!
//! The `data` table is `UNIQUE(book, format)` — one file per format per
//! book. Two byte-identical PDFs therefore *cannot* be "one book with
//! two PDF formats"; the schema has nowhere to put the second. They are
//! one file that exists twice, which is a duplicate to resolve. Both
//! stay tracked either way: the app does not get to decide which of
//! somebody's files matters.
//!
//! # Why "specific" is load-bearing
//!
//! A scanner-produced PDF usually has no Info dictionary at all, or a
//! title like `Microsoft Word - untitled`. An unguarded "the metadata
//! matches" rule would merge every untitled scan in the folder into one
//! book — in exactly the case this whole model exists for. So metadata
//! counts as evidence only when it is [`CandidateMetadata::is_specific`]:
//! a real title that is not boilerplate, plus at least one of an author
//! or an ISBN.

use std::collections::HashMap;

/// Titles that carry no information, whatever they say.
///
/// Every one of these is something a tool wrote, not something a person
/// chose. Matched after normalisation, and `starts_with` for the two
/// that carry a document name after a fixed prefix.
const BOILERPLATE_TITLES: &[&str] = &["untitled", "untitled document", "document", "document1", "unknown", "unknown book", "scanned document", "scan", "powerpoint presentation", "presentation", "book1", "no title", "none", "null"];

/// Prefixes an office suite or a driver puts in front of a filename.
const BOILERPLATE_PREFIXES: &[&str] = &["microsoft word - ", "microsoft powerpoint - ", "untitled - ", "adobe acrobat - "];

/// What is known about one file's own embedded metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateMetadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub isbn: Option<String>,
}

impl CandidateMetadata {
    /// Whether this metadata is specific enough to group two files on.
    ///
    /// A title alone is not enough. Two different scans both titled
    /// `Report` would merge, and so would every PDF whose producer
    /// wrote the filename into the title field — which is why a title
    /// equal to the file's own name does not count either. The
    /// filename is explicitly not evidence, so a title that merely
    /// repeats it adds nothing.
    pub fn is_specific(&self, stem: &str) -> bool {
        let Some(title) = self.title.as_deref().map(normalise) else { return false };
        if title.is_empty() || is_boilerplate(&title) {
            return false;
        }
        if title == normalise(stem) {
            return false;
        }
        let has_author = self.authors.iter().any(|a| {
            let a = normalise(a);
            !a.is_empty() && a != "unknown" && a != "unknown author"
        });
        has_author || self.isbn.as_deref().is_some_and(|i| !i.trim().is_empty())
    }

    /// The comparison key two files must share to be one book.
    ///
    /// An ISBN alone is enough when both have one — it identifies an
    /// edition, which is exactly the question. Otherwise it is
    /// title-and-authors.
    fn identity_key(&self) -> Option<String> {
        if let Some(isbn) = self.isbn.as_deref() {
            let isbn: String = isbn.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
            if !isbn.is_empty() {
                return Some(format!("isbn:{isbn}"));
            }
        }
        let title = self.title.as_deref().map(normalise)?;
        let mut authors: Vec<String> = self.authors.iter().map(|a| normalise(a)).filter(|a| !a.is_empty()).collect();
        authors.sort();
        Some(format!("ta:{title}|{}", authors.join("&")))
    }
}

/// One file the scan found, as far as grouping is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Opaque identifier — the scan uses the file's relative path.
    pub id: String,
    /// The filename without its extension. Used *only* to reject a
    /// title that merely repeats it; never to group.
    pub stem: String,
    /// Lowercase, no dot.
    pub extension: String,
    /// `None` until the hashing pass has reached this file.
    pub hash: Option<String>,
    pub metadata: CandidateMetadata,
}

/// Two files that are the same, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicatePair {
    pub first: String,
    pub second: String,
    pub reason: DuplicateReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateReason {
    /// Byte-identical.
    SameContent,
    /// Metadata says one book, and they are the same format — so the
    /// schema has room for only one of them.
    SameBookSameFormat,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Grouping {
    /// One entry per book, holding the ids of its format files. A
    /// single-file book is a one-element group.
    pub books: Vec<Vec<String>>,
    /// Files that are the same as another file. Both remain in
    /// [`Grouping::books`] — flagging a duplicate is not deciding which
    /// copy to throw away.
    pub duplicates: Vec<DuplicatePair>,
}

/// Groups candidates into books and reports duplicates.
///
/// Order-independent: the same set of candidates in any order produces
/// the same grouping, because two machines scanning the same folder must
/// reach the same conclusions.
pub fn group(candidates: &[Candidate]) -> Grouping {
    let mut sorted: Vec<&Candidate> = candidates.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    let mut grouping = Grouping::default();

    // Byte-identical files first. This is the strongest evidence there
    // is and it does not depend on anyone having filled in metadata.
    let mut by_hash: HashMap<&str, Vec<&Candidate>> = HashMap::new();
    for candidate in &sorted {
        if let Some(hash) = candidate.hash.as_deref() {
            by_hash.entry(hash).or_default().push(candidate);
        }
    }
    let mut hash_keys: Vec<&&str> = by_hash.keys().collect();
    hash_keys.sort();
    for hash in hash_keys {
        let same = &by_hash[*hash];
        for pair in same.windows(2) {
            grouping.duplicates.push(DuplicatePair { first: pair[0].id.clone(), second: pair[1].id.clone(), reason: DuplicateReason::SameContent });
        }
    }

    // Then metadata, but only where it is specific enough to mean
    // something.
    let mut by_identity: HashMap<String, Vec<&Candidate>> = HashMap::new();
    let mut ungrouped: Vec<&Candidate> = Vec::new();
    for candidate in &sorted {
        match candidate.metadata.identity_key().filter(|_| candidate.metadata.is_specific(&candidate.stem)) {
            Some(key) => by_identity.entry(key).or_default().push(candidate),
            None => ungrouped.push(candidate),
        }
    }

    let mut identity_keys: Vec<&String> = by_identity.keys().collect();
    identity_keys.sort();
    for key in identity_keys {
        let same_book = &by_identity[key];
        // Within one book, one file per format. A second file of the
        // same format is a duplicate, not another format, because
        // `data` is UNIQUE(book, format).
        let mut by_format: HashMap<&str, Vec<&Candidate>> = HashMap::new();
        for candidate in same_book {
            by_format.entry(candidate.extension.as_str()).or_default().push(candidate);
        }

        let mut book: Vec<String> = Vec::new();
        let mut formats: Vec<&&str> = by_format.keys().collect();
        formats.sort();
        for format in formats {
            let same_format = &by_format[*format];
            book.push(same_format[0].id.clone());
            for extra in &same_format[1..] {
                // Already reported if they are byte-identical; this
                // catches two different files claiming to be the same
                // edition in the same format.
                let already = grouping.duplicates.iter().any(|d| (d.first == same_format[0].id && d.second == extra.id) || (d.second == same_format[0].id && d.first == extra.id));
                if !already {
                    grouping.duplicates.push(DuplicatePair { first: same_format[0].id.clone(), second: extra.id.clone(), reason: DuplicateReason::SameBookSameFormat });
                }
                // Still tracked, as its own book: the app does not get
                // to silently drop one of somebody's files.
                grouping.books.push(vec![extra.id.clone()]);
            }
        }
        grouping.books.push(book);
    }

    for candidate in ungrouped {
        grouping.books.push(vec![candidate.id.clone()]);
    }

    grouping.books.sort();
    grouping.duplicates.sort_by(|a, b| (&a.first, &a.second).cmp(&(&b.first, &b.second)));
    grouping
}

/// Lowercase, collapse whitespace, drop punctuation that varies between
/// otherwise identical metadata.
fn normalise(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_space = true;
    for ch in value.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim().to_string()
}

fn is_boilerplate(normalised_title: &str) -> bool {
    if BOILERPLATE_TITLES.contains(&normalised_title) {
        return true;
    }
    // Normalisation has already stripped the punctuation these prefixes
    // carry, so compare against normalised forms.
    if BOILERPLATE_PREFIXES.iter().any(|p| normalised_title.starts_with(&normalise(p))) {
        return true;
    }
    // A "title" that is nothing but digits is a page count, a date or a
    // scanner counter, never a book.
    !normalised_title.is_empty() && normalised_title.chars().all(|c| c.is_ascii_digit() || c == ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, ext: &str) -> Candidate {
        Candidate { id: id.into(), stem: id.trim_end_matches(&format!(".{ext}")).into(), extension: ext.into(), hash: None, metadata: CandidateMetadata::default() }
    }

    fn with_meta(id: &str, ext: &str, title: &str, author: &str) -> Candidate {
        let mut c = candidate(id, ext);
        c.metadata = CandidateMetadata { title: Some(title.into()), authors: vec![author.into()], isbn: None };
        c
    }

    fn books(grouping: &Grouping) -> Vec<Vec<&str>> {
        grouping.books.iter().map(|b| b.iter().map(String::as_str).collect()).collect()
    }

    // ---- the headline rule ----

    /// The departure from calibre and from this crate's own
    /// `find_books_in_directory`: a shared filename means nothing.
    #[test]
    fn matching_filenames_alone_do_not_make_one_book() {
        let files = vec![candidate("Dune.epub", "epub"), candidate("Dune.pdf", "pdf")];
        assert_eq!(books(&group(&files)), vec![vec!["Dune.epub"], vec!["Dune.pdf"]]);
    }

    #[test]
    fn specific_matching_metadata_and_different_formats_make_one_book() {
        let files = vec![with_meta("a.epub", "epub", "Dune", "Frank Herbert"), with_meta("b.pdf", "pdf", "Dune", "Frank Herbert")];
        assert_eq!(books(&group(&files)), vec![vec!["a.epub", "b.pdf"]]);
        assert!(group(&files).duplicates.is_empty());
    }

    /// `data` is UNIQUE(book, format), so the schema has nowhere to put
    /// a second PDF of one book.
    #[test]
    fn specific_matching_metadata_and_the_same_format_is_a_duplicate() {
        let files = vec![with_meta("a.pdf", "pdf", "Dune", "Frank Herbert"), with_meta("b.pdf", "pdf", "Dune", "Frank Herbert")];
        let grouping = group(&files);

        assert_eq!(grouping.duplicates.len(), 1);
        assert_eq!(grouping.duplicates[0].reason, DuplicateReason::SameBookSameFormat);
        // Both still tracked -- flagging a duplicate is not choosing
        // which of somebody's files to discard.
        assert_eq!(books(&grouping), vec![vec!["a.pdf"], vec!["b.pdf"]]);
    }

    #[test]
    fn byte_identical_files_are_a_duplicate_whatever_their_metadata() {
        let mut a = candidate("Receipts/invoice.pdf", "pdf");
        let mut b = candidate("Archive/invoice.pdf", "pdf");
        a.hash = Some("samehash".into());
        b.hash = Some("samehash".into());

        let grouping = group(&[a, b]);
        assert_eq!(grouping.duplicates.len(), 1);
        assert_eq!(grouping.duplicates[0].reason, DuplicateReason::SameContent);
        assert_eq!(grouping.duplicates[0].first, "Archive/invoice.pdf");
        assert_eq!(books(&grouping).len(), 2, "both files stay tracked");
    }

    #[test]
    fn different_content_is_not_a_duplicate() {
        let mut a = candidate("a.pdf", "pdf");
        let mut b = candidate("b.pdf", "pdf");
        a.hash = Some("one".into());
        b.hash = Some("two".into());
        assert!(group(&[a, b]).duplicates.is_empty());
    }

    /// A duplicate is not reported twice for the same pair just because
    /// two rules both noticed it.
    #[test]
    fn content_and_metadata_agreeing_reports_one_duplicate_not_two() {
        let mut a = with_meta("a.pdf", "pdf", "Dune", "Frank Herbert");
        let mut b = with_meta("b.pdf", "pdf", "Dune", "Frank Herbert");
        a.hash = Some("same".into());
        b.hash = Some("same".into());

        let grouping = group(&[a, b]);
        assert_eq!(grouping.duplicates.len(), 1, "{:?}", grouping.duplicates);
        assert_eq!(grouping.duplicates[0].reason, DuplicateReason::SameContent);
    }

    // ---- "specific" is load-bearing ----

    /// The failure this guard exists to prevent, in the exact case the
    /// tracked-folder model was built for.
    #[test]
    fn untitled_scans_do_not_all_merge_into_one_book() {
        let mut files = Vec::new();
        for i in 0..5 {
            let mut c = candidate(&format!("scan000{i}.pdf"), "pdf");
            // What a scanner actually writes.
            c.metadata = CandidateMetadata { title: Some("Microsoft Word - untitled".into()), authors: vec![], isbn: None };
            files.push(c);
        }
        assert_eq!(group(&files).books.len(), 5, "every untitled scan merged into one book");
    }

    #[test]
    fn a_title_with_no_author_or_isbn_is_not_specific_enough() {
        let mut a = candidate("a.pdf", "pdf");
        let mut b = candidate("b.epub", "epub");
        a.metadata = CandidateMetadata { title: Some("Report".into()), authors: vec![], isbn: None };
        b.metadata = CandidateMetadata { title: Some("Report".into()), authors: vec![], isbn: None };
        // Two unrelated documents both called "Report" is completely
        // ordinary.
        assert_eq!(group(&[a, b]).books.len(), 2);
    }

    /// The filename is explicitly not evidence, so a title that merely
    /// repeats it adds nothing -- which is what many PDF producers write.
    #[test]
    fn a_title_that_just_repeats_the_filename_is_not_evidence() {
        let mut a = candidate("scan0001.pdf", "pdf");
        let mut b = candidate("scan0001.txt", "txt");
        for c in [&mut a, &mut b] {
            c.metadata = CandidateMetadata { title: Some("scan0001".into()), authors: vec!["HP Scanner".into()], isbn: None };
        }
        assert_eq!(group(&[a, b]).books.len(), 2);
    }

    #[test]
    fn an_isbn_is_enough_without_an_author() {
        let mut a = candidate("a.epub", "epub");
        let mut b = candidate("b.pdf", "pdf");
        for c in [&mut a, &mut b] {
            c.metadata = CandidateMetadata { title: Some("Dune".into()), authors: vec![], isbn: Some("978-0-441-01359-3".into()) };
        }
        assert_eq!(books(&group(&[a, b])), vec![vec!["a.epub", "b.pdf"]]);
    }

    #[test]
    fn an_isbn_matches_across_punctuation_differences() {
        let mut a = candidate("a.epub", "epub");
        let mut b = candidate("b.pdf", "pdf");
        a.metadata = CandidateMetadata { title: Some("Dune".into()), authors: vec![], isbn: Some("978-0-441-01359-3".into()) };
        b.metadata = CandidateMetadata { title: Some("Dune, or the Desert".into()), authors: vec![], isbn: Some("9780441013593".into()) };
        assert_eq!(books(&group(&[a, b])), vec![vec!["a.epub", "b.pdf"]], "an ISBN identifies the edition even when titles differ");
    }

    #[test]
    fn an_author_of_unknown_is_not_an_author() {
        let mut a = with_meta("a.epub", "epub", "Some Book", "Unknown");
        let mut b = with_meta("b.pdf", "pdf", "Some Book", "Unknown");
        a.stem = "a".into();
        b.stem = "b".into();
        assert_eq!(group(&[a, b]).books.len(), 2);
    }

    #[test]
    fn boilerplate_titles_are_recognised() {
        for title in ["Untitled", "untitled document", "Document1", "Microsoft Word - Report", "Scanned Document", "  ", "2024", "PowerPoint Presentation"] {
            let meta = CandidateMetadata { title: Some(title.into()), authors: vec!["A Real Author".into()], isbn: None };
            assert!(!meta.is_specific("somefile"), "{title:?} should not count as a specific title");
        }
    }

    #[test]
    fn a_real_title_with_a_real_author_is_specific() {
        let meta = CandidateMetadata { title: Some("Nineteen Eighty-Four".into()), authors: vec!["George Orwell".into()], isbn: None };
        assert!(meta.is_specific("scan0001"));
    }

    // ---- matching tolerances ----

    #[test]
    fn metadata_matches_across_case_and_punctuation() {
        let a = with_meta("a.epub", "epub", "Nineteen Eighty-Four", "George Orwell");
        let b = with_meta("b.pdf", "pdf", "nineteen eighty four", "george  orwell");
        assert_eq!(books(&group(&[a, b])), vec![vec!["a.epub", "b.pdf"]]);
    }

    #[test]
    fn author_order_does_not_matter() {
        let mut a = candidate("a.epub", "epub");
        let mut b = candidate("b.pdf", "pdf");
        a.metadata = CandidateMetadata { title: Some("Good Omens".into()), authors: vec!["Terry Pratchett".into(), "Neil Gaiman".into()], isbn: None };
        b.metadata = CandidateMetadata { title: Some("Good Omens".into()), authors: vec!["Neil Gaiman".into(), "Terry Pratchett".into()], isbn: None };
        assert_eq!(books(&group(&[a, b])), vec![vec!["a.epub", "b.pdf"]]);
    }

    #[test]
    fn different_books_by_one_author_stay_separate() {
        let a = with_meta("a.epub", "epub", "Dune", "Frank Herbert");
        let b = with_meta("b.epub", "epub", "Dune Messiah", "Frank Herbert");
        assert_eq!(group(&[a, b]).books.len(), 2);
    }

    /// Two machines scanning one folder must reach the same conclusions,
    /// so the result cannot depend on directory-listing order.
    #[test]
    fn grouping_is_independent_of_input_order() {
        let files = vec![
            with_meta("c.pdf", "pdf", "Dune", "Frank Herbert"),
            candidate("z.pdf", "pdf"),
            with_meta("a.epub", "epub", "Dune", "Frank Herbert"),
            candidate("b.pdf", "pdf"),
        ];
        let forward = group(&files);
        let mut reversed = files.clone();
        reversed.reverse();
        assert_eq!(forward, group(&reversed));
    }

    #[test]
    fn an_unhashed_file_is_simply_not_matched_on_content() {
        // Phase one has no hashes yet; grouping still has to work, just
        // with less evidence.
        let files = vec![candidate("a.pdf", "pdf"), candidate("b.pdf", "pdf")];
        let grouping = group(&files);
        assert!(grouping.duplicates.is_empty());
        assert_eq!(grouping.books.len(), 2);
    }

    #[test]
    fn nothing_in_means_nothing_out() {
        assert_eq!(group(&[]), Grouping::default());
    }

    #[test]
    fn three_byte_identical_files_report_a_chain_of_pairs() {
        let mut files = Vec::new();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            let mut c = candidate(name, "pdf");
            c.hash = Some("same".into());
            files.push(c);
        }
        // Two pairs, not three: enough to link all three together for
        // review without reporting every combination.
        assert_eq!(group(&files).duplicates.len(), 2);
    }
}
