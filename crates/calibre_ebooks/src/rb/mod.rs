//! Port of `calibre.ebooks.rb`: the RocketBook (`.rb`) e-book format --
//! a NuvoMedia/Gemstar Rocket eBook container holding zlib-compressed
//! HTML "pages" and raw PNG images behind a small binary TOC.
//!
//! - `header.rs` / `reader.rs`: port of `reader.py` (`Reader`).
//! - `writer.rs`: port of `writer.py` (`RBWriter`).
//! - `rbml.rs`: port of `rbml.py` (`RBMLizer`).
//! - This file: port of `__init__.py` (`HEADER`, `RocketBookError`,
//!   `unique_name`).

pub mod header;
pub mod rbml;
pub mod reader;
pub mod writer;

pub use header::MAGIC as HEADER;

/// Port of `RocketBookError`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RocketBookError {
    #[error("Could not read file. Does not contain a valid RocketBook Header.")]
    InvalidHeader,
    #[error(
        "File is corrupt. The file size recorded in the header does not match the actual file size."
    )]
    SizeMismatch,
}

/// `os.path.splitext(name)[1]` -- the extension *including* its leading
/// dot, or `""` if there isn't one. A leading run of dots (e.g.
/// `".bashrc"`) is not itself treated as an extension.
fn splitext_ext(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut start = 0;
    while start < chars.len() && chars[start] == '.' {
        start += 1;
    }
    let mut last_dot = None;
    for (idx, ch) in chars.iter().enumerate().skip(start) {
        if *ch == '.' {
            last_dot = Some(idx);
        }
    }
    match last_dot {
        Some(idx) => chars[idx..].iter().collect(),
        None => String::new(),
    }
}

/// Port of `unique_name(name, used_names)`.
///
/// # Deviation from upstream
///
/// Python's fallback branch (taken when `name` is already 32+ chars, or
/// already used) builds each candidate as
/// `'{}-{}.{}'.format(str(i).rjust('0', 4)[:4], base_name, ext)`. But
/// `str.rjust`'s real signature is `rjust(width, fillchar=' ')` -- that
/// call passes the *string* `'0'` as `width` and the *int* `4` as
/// `fillchar`, which raises `TypeError: 'str' object cannot be
/// interpreted as an integer` in real Python the instant this branch
/// runs. It is therefore dead-on-arrival upstream: any name that would
/// actually need the fallback crashes instead of producing one.
///
/// Rather than reproduce a guaranteed crash, this port implements the
/// evidently-intended piece instead -- a 4-digit zero-padded counter
/// (`f'{i:04}'`, what `str(i).rjust(4, '0')` -- the call that was
/// clearly meant here -- would have produced) -- while otherwise
/// following the surrounding logic exactly, including its other real
/// (non-crashing) quirk: `ext = os.path.splitext(name)[1][:3]` keeps
/// the extension's leading dot as one of the 3 characters it keeps, so
/// a `.png` extension is truncated to `.pn`, and the candidate template
/// itself (`"{i:04}-{base_name}.{ext}"`) adds *another* literal dot in
/// front of that already-dotted `ext`, so real (non-crash) upstream
/// output for `"taken.png"` looks like `"0000-taken.png..pn"`, not the
/// tidy `"0000-taken.png"` one might expect. Preserved as-is.
pub fn unique_name(name: &str, used_names: &[String]) -> String {
    let name = std::path::Path::new(name)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());

    if name.chars().count() < 32 && !used_names.iter().any(|u| u == &name) {
        return name;
    }

    let ext: String = splitext_ext(&name).chars().take(3).collect();
    let base_name: String = name.chars().take(22).collect();

    let mut candidate = String::new();
    for i in 0..9999u32 {
        candidate = format!("{i:04}-{base_name}.{ext}");
        if !used_names.iter().any(|u| u == &candidate) {
            break;
        }
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_unused_name_passes_through() {
        assert_eq!(unique_name("index.html", &[]), "index.html");
    }

    #[test]
    fn basename_is_stripped() {
        assert_eq!(unique_name("dir/sub/index.html", &[]), "index.html");
    }

    #[test]
    fn used_name_falls_back_to_a_zero_padded_counter_and_the_truncated_dotted_extension_quirk() {
        let used = vec!["taken.png".to_string()];
        let out = unique_name("taken.png", &used);
        // See the module docs: `ext` keeps its own leading dot (".pn"),
        // and the template adds a second literal dot in front of it.
        assert_eq!(out, "0000-taken.png..pn");
    }

    #[test]
    fn long_name_falls_back_to_a_zero_padded_counter() {
        let long_name = "a".repeat(40) + ".png";
        let out = unique_name(&long_name, &[]);
        assert!(out.starts_with("0000-"), "{out}");
        assert!(out.ends_with(".pn"), "{out}");
    }

    #[test]
    fn dedup_increments_the_counter() {
        let long_name = "b".repeat(40);
        let first = unique_name(&long_name, &[]);
        let used = vec![first.clone()];
        let second = unique_name(&long_name, &used);
        assert_ne!(first, second);
        assert!(second.starts_with("0001-"), "{second}");
    }

    #[test]
    fn splitext_ext_handles_dotfiles_and_no_extension() {
        assert_eq!(splitext_ext("index.html"), ".html");
        assert_eq!(splitext_ext("noext"), "");
        assert_eq!(splitext_ext(".bashrc"), "");
    }
}
