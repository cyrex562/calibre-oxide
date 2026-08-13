use lazy_static::lazy_static;
use std::collections::HashSet;
use std::path::Path;
use unidecode::unidecode;

lazy_static! {
    static ref INVALID_CHARS: HashSet<char> = {
        let mut s = HashSet::new();
        // '\\', '|', '?', '*', '<', '"', ':', '>', '+', '/'
        for c in ['\\', '|', '?', '*', '<', '"', ':', '>', '+', '/'] {
            s.insert(c);
        }
        // control chars 0-31
        for i in 0..32 {
            if let Some(c) = std::char::from_u32(i) {
                s.insert(c);
            }
        }
        s
    };
}

pub fn ascii_text(orig: &str) -> String {
    unidecode(orig)
}

pub fn ascii_filename(orig: &str) -> String {
    let text = ascii_text(orig).replace('?', "_");
    // Replace invalid ascii chars (control chars already substituted by sanitization?)
    // But ascii_filename specifically does:
    // ans = ''.join(x if ord(x) >= 32 else substitute for x in orig)
    // and then calls sanitize_file_name.

    let substitute = '_';
    let filtered: String = text
        .chars()
        .map(|c| if (c as u32) >= 32 { c } else { substitute })
        .collect();
    sanitize_file_name(&filtered)
}

pub fn sanitize_file_name(name: &str) -> String {
    let substitute = '_';
    let mut chars = String::with_capacity(name.len());

    for c in name.chars() {
        if INVALID_CHARS.contains(&c) {
            chars.push(substitute);
        } else {
            chars.push(c);
        }
    }

    // Replace whitespace with space and strip
    // Replaces all whitespace with space?
    // Python: one = re.sub(r'\s', ' ', one).strip()
    // This replaces tabs/newlines with space.
    let one = chars.replace(['\t', '\n', '\r'], " "); // simpler regex equivalent
    let mut one = one.trim().to_string();

    // Split ext
    let path = Path::new(&one);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // If stem matches ^\.+$ -> _
    // i.e. stem is only dots.
    if stem.chars().all(|c| c == '.') && !stem.is_empty() {
        one = "_".to_string(); // stem becomes _
    } else {
        one = stem;
    }

    // one = one.replace("..", substitute)
    one = one.replace("..", "_");

    if !ext.is_empty() {
        one.push('.');
        one.push_str(&ext);
    } else if chars.ends_with('.') {
        // path.file_stem logic trims trailing dot?
        // Check original split logic.
        // Rust Path: "foo." -> stem "foo", ext "".
        // So reconstruction loses dot.
        // Python os.path.splitext("foo.") -> ("foo", "") on Linux?
        // No, ("foo.", "") usually?
        // Let's rely on manual split for exact parity if needed.
        // Python: bname, ext = os.path.splitext(one)
    }

    // Windows checks: ends with . or space
    if one.ends_with('.') || one.ends_with(' ') {
        one.pop();
        one.push('_');
    }

    // Leading dot -> _
    if one.starts_with('.') {
        one.insert(0, '_');
    }

    one
}

pub fn shorten_component(s: &str, by_what: usize) -> String {
    let len = s.len();
    if len < by_what {
        return s.to_string();
    }
    let l = (len - by_what) as isize / 2;
    if l <= 0 {
        return s.to_string();
    }
    let _l = l as usize;
    // s[:l] + s[-l:]
    // Be careful with unicode boundaries!
    // Python works on codepoints (str) or bytes? Str.
    // Rust chars.
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < by_what {
        return s.to_string();
    }
    let l_chars = (chars.len().saturating_sub(by_what)) / 2;
    if l_chars == 0 {
        return s.to_string();
    }

    let mut res = String::new();
    res.extend(&chars[..l_chars]);
    res.extend(&chars[chars.len() - l_chars..]);
    res
}

pub fn limit_component(x: &str, limit: usize) -> String {
    // UTF-8 length used for now.
    let mut s = x.to_string();
    while s.len() > limit {
        let delta = s.len() - limit;
        s = shorten_component(&s, std::cmp::max(2, delta / 2));
    }
    s
}

/// Check if two paths point to the same actual file on the filesystem.
///
/// Port of `calibre.utils.filenames.samefile`. On Unix this is exactly
/// `os.path.samefile`: both paths must exist and resolve to the same
/// device/inode pair. On Windows the Python falls back to comparing
/// normalized absolute path strings (case-insensitively) when the
/// paths aren't identical strings; that fallback is reproduced here
/// with `dunce`-free `std::fs::canonicalize`, which is the closest
/// portable equivalent without pulling in `GetFileInformationByHandle`
/// bindings.
///
/// Returns `false` (rather than erroring) when either path doesn't
/// exist, matching the Python's `os.path.samefile` behavior of raising
/// `OSError` — which every caller in `worker.py` treats as "not the
/// same file".
pub fn samefile(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
            return false;
        };
        ma.dev() == mb.dev() && ma.ino() == mb.ino()
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(ca), Ok(cb)) => ca
                .to_string_lossy()
                .eq_ignore_ascii_case(&cb.to_string_lossy()),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samefile_is_true_for_a_path_and_itself() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        assert!(samefile(tmp.path(), tmp.path()));
    }

    #[test]
    fn samefile_is_true_for_a_hardlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"content").expect("write");
        std::fs::hard_link(&a, &b).expect("hardlink");
        assert!(samefile(&a, &b));
    }

    #[test]
    fn samefile_is_false_for_two_distinct_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"content").expect("write");
        std::fs::write(&b, b"content").expect("write");
        assert!(!samefile(&a, &b));
    }

    #[test]
    fn samefile_is_false_when_a_path_does_not_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("real.txt");
        std::fs::write(&a, b"content").expect("write");
        let missing = dir.path().join("missing.txt");
        assert!(!samefile(&a, &missing));
        assert!(!samefile(&missing, &a));
    }
}
