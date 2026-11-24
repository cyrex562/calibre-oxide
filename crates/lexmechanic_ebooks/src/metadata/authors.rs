use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{HashMap, HashSet};

lazy_static! {
    static ref AUTHORS_SPLIT_REGEX: Regex = Regex::new(r"(?i),?\s+(and|with)\s+").unwrap();
    static ref AUTHOR_NAME_SUFFIXES: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.extend([
            "Jr", "Sr", "Inc", "Ph.D", "Phd", "MD", "M.D", "I", "II", "III", "IV", "Junior",
            "Senior",
        ]);
        s
    };
    static ref AUTHOR_NAME_PREFIXES: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.extend(["Mr", "Mrs", "Ms", "Dr", "Prof"]);
        s
    };
    static ref AUTHOR_NAME_COPYWORDS: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.extend([
            "Agency",
            "Corporation",
            "Company",
            "Co.",
            "Council",
            "Committee",
            "Inc.",
            "Institute",
            "National",
            "Society",
            "Club",
            "Team",
            "Software",
            "Games",
            "Entertainment",
            "Media",
            "Studios",
        ]);
        s
    };
    static ref AUTHOR_SURNAME_PREFIXES: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.extend(["da", "de", "di", "la", "le", "van", "von"]);
        s
    };
}

const AUTHOR_SORT_COPY_METHOD: &str = "comma";
const AUTHOR_USE_SURNAME_PREFIXES: bool = false;

pub fn string_to_authors(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        return Vec::new();
    }
    let raw = raw.replace("&&", "\u{ffff}");
    let raw = AUTHORS_SPLIT_REGEX.replace_all(&raw, "&");
    raw.split('&')
        .map(|a| a.trim().replace("\u{ffff}", "&"))
        .filter(|a| !a.is_empty())
        .collect()
}

pub fn authors_to_string(authors: &[String]) -> String {
    authors
        .iter()
        .map(|a| a.replace('&', "&&"))
        .filter(|a| !a.is_empty())
        .collect::<Vec<_>>()
        .join(" & ")
}

pub fn remove_bracketed_text(src: &str) -> String {
    // Basic implementation mimicking Python's logic
    // Recursively removing brackets () [] {}
    let text = src.to_string();
    // Brackets map: ( -> ), [ -> ], { -> }
    // Python implementation uses a single scan with counters.
    // Let's implement that:

    let chars: Vec<char> = text.chars().collect();
    let mut buf = String::new();
    let mut counters = HashMap::new();
    counters.insert('(', 0);
    counters.insert('[', 0);
    counters.insert('{', 0);

    let pairs = HashMap::from([(')', '('), (']', '['), ('}', '{')]);

    let mut total = 0;

    for c in chars {
        if counters.contains_key(&c) {
            *counters.get_mut(&c).unwrap() += 1;
            total += 1;
        } else if let Some(opener) = pairs.get(&c) {
            if counters[opener] > 0 {
                *counters.get_mut(opener).unwrap() -= 1;
                total -= 1;
            }
        } else if total < 1 {
            buf.push(c);
        }
    }

    buf
}

pub fn author_to_author_sort(
    author: &str,
    method: Option<&str>,
    copywords: Option<&HashSet<String>>,
    use_surname_prefixes: Option<bool>,
    surname_prefixes: Option<&HashSet<String>>,
    name_prefixes: Option<&HashSet<String>>,
    name_suffixes: Option<&HashSet<String>>,
) -> String {
    if author.is_empty() {
        return String::new();
    }

    let method = method.unwrap_or(AUTHOR_SORT_COPY_METHOD);
    if method == "copy" {
        return author.to_string();
    }

    let sauthor = remove_bracketed_text(author);
    let sauthor = sauthor.trim();
    if method == "comma" && sauthor.contains(',') {
        return author.to_string();
    }

    let tokens: Vec<&str> = sauthor.split_whitespace().collect();
    if tokens.len() < 2 {
        return author.to_string();
    }

    // Convert to lowercase set for checking
    let ltoks: HashSet<String> = tokens.iter().map(|s| s.to_lowercase()).collect();

    // Check copywords
    // Using default if None
    let default_copywords: HashSet<String> = AUTHOR_NAME_COPYWORDS
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let copy_words = match copywords {
        Some(cw) => cw,
        None => &default_copywords,
    };

    if !ltoks.is_disjoint(copy_words) {
        return author.to_string();
    }

    let use_prefixes = use_surname_prefixes.unwrap_or(AUTHOR_USE_SURNAME_PREFIXES);

    let default_surname_prefixes: HashSet<String> = AUTHOR_SURNAME_PREFIXES
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let author_surname_prefixes = match surname_prefixes {
        Some(sp) => sp,
        None => &default_surname_prefixes,
    };

    if use_prefixes {
        if tokens.len() == 2 && author_surname_prefixes.contains(&tokens[0].to_lowercase()) {
            return author.to_string();
        }
    }

    // Prefixes (Mr, Mrs, etc)
    let default_name_prefixes: HashSet<String> = AUTHOR_NAME_PREFIXES
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let name_prefixes_set = match name_prefixes {
        Some(np) => np,
        None => &default_name_prefixes,
    };
    // Include with dot
    let mut prefixes_final = name_prefixes_set.clone();
    for p in name_prefixes_set {
        prefixes_final.insert(format!("{}.", p));
    }

    let mut first_idx = 0;
    for (i, token) in tokens.iter().enumerate() {
        if !prefixes_final.contains(&token.to_lowercase()) {
            first_idx = i;
            break;
        }
    }
    // else return author (all prefixes?)
    // Python: for ... else return author
    if first_idx == 0 && prefixes_final.contains(&tokens[0].to_lowercase()) {
        // loop finished without breaking? Wait.
        // Python loop: for first in range(len(tokens)): if not in prefix: break
        // else: return author (meaning ALL were prefixes)
        // If break happened, first is set.

        // In Rust: if loop finishes without finding non-prefix
        let all_prefixes = tokens
            .iter()
            .all(|t| prefixes_final.contains(&t.to_lowercase()));
        if all_prefixes {
            return author.to_string();
        }
    }

    // Suffixes
    let default_name_suffixes: HashSet<String> = AUTHOR_NAME_SUFFIXES
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let name_suffixes_set = match name_suffixes {
        Some(ns) => ns,
        None => &default_name_suffixes,
    };
    let mut suffixes_final = name_suffixes_set.clone();
    for s in name_suffixes_set {
        suffixes_final.insert(format!("{}.", s));
    }

    let mut last_idx = tokens.len() - 1;
    // Python range: len(tokens) - 1, first - 1, -1 (down to first)
    // if not in suffixes: break
    // else: return author (all after first were suffixes?)

    let mut found_non_suffix = false;
    for i in (first_idx..tokens.len()).rev() {
        if !suffixes_final.contains(&tokens[i].to_lowercase()) {
            last_idx = i;
            found_non_suffix = true;
            break;
        }
    }
    if !found_non_suffix {
        // Means everything from first to end was a suffix?
        return author.to_string();
    }

    let suffix_str = tokens[last_idx + 1..].join(" ");

    // Surname prefixes (von, van, etc)
    // Python: if last > first and tokens[last - 1] in author_surname_prefixes:
    // tokens[last-1] += ' ' + tokens[last]
    // last -= 1

    // We need mutable tokens logic, but we have slice of strs.
    // Let's reconstruct.
    let mut token_list: Vec<String> = tokens.iter().map(|s| s.to_string()).collect();

    if use_prefixes {
        if last_idx > first_idx
            && author_surname_prefixes.contains(&token_list[last_idx - 1].to_lowercase())
        {
            let combined = format!("{} {}", token_list[last_idx - 1], token_list[last_idx]);
            token_list[last_idx - 1] = combined;
            // We conceptually remove last_idx token or move last_idx back
            last_idx -= 1;
            // But Wait, token_list has N items. We merged (last-1) and (last).
            // effective item at last-1 is now the surname.
            // We should effectively duplicate python logic string construction.
        }
    }

    // atokens = tokens[last:last+1] + tokens[first:last]
    // In Rust with our mutable logic:
    // Actually, if we merged, the token at [last_idx] is the surname (which was last-1 before merge, so index matches if we consider last_idx decremented)

    // Wait, Python:
    // tokens[last - 1] += ' ' + tokens[last]
    // last -= 1
    // atokens = tokens[last:last+1] ...

    // If merged:
    // old tokens: [A, B, von, C]
    // first=0, last=3 (C).
    // if von in prefixes:
    // tokens[2] becomes "von C"
    // last becomes 2.
    // atokens = tokens[2:3] ("von C") + tokens[0:2] (A, B)
    // Result: von C, A B

    let surname = token_list[last_idx].clone();
    let mut atokens = Vec::new();
    atokens.push(surname);

    // Add rest (first..last)
    // Note: if we merged, we should not include the *original* last token separately?
    // In Python, tokens list was modified in place.
    // Since we modified tokens_list, and decremented last_idx,
    // tokens[first:last] will exclude the consolidated surname.

    for i in first_idx..last_idx {
        atokens.push(token_list[i].clone());
    }

    // Append suffix_str if any
    if !suffix_str.is_empty() {
        atokens.push(suffix_str);
    }

    // Add comma
    let num_toks = atokens.len();
    if method != "nocomma" && num_toks > 1 {
        atokens[0].push(',');
    }

    atokens.join(" ")
}

pub fn authors_to_sort_string(authors: &[String]) -> String {
    authors
        .iter()
        .map(|a| author_to_author_sort(a, None, None, None, None, None, None))
        .collect::<Vec<_>>()
        .join(" & ")
}
