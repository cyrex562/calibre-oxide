use std::collections::HashMap;
use url::form_urlencoded;

lazy_static::lazy_static! {
    static ref AUTHOR_SEARCHES: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("goodreads", "https://www.goodreads.com/book/author/{author}");
        m.insert("wikipedia", "https://en.wikipedia.org/w/index.php?search={author}");
        m.insert("google", "https://www.google.com/search?tbm=bks&q=inauthor:%22{author}%22");
        m.insert("amzn", "https://www.amazon.com/gp/search/ref=sr_adv_b/?search-alias=stripbooks&unfiltered=1&field-author={author}&sort=relevanceexprank");
        m
    };

    static ref BOOK_SEARCHES: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("goodreads", "https://www.goodreads.com/search?q={author}+{title}&search%5Bsource%5D=goodreads&search_type=books&tab=books");
        m.insert("google", "https://www.google.com/search?tbm=bks&q=inauthor:%22{author}%22+intitle:%22{title}%22");
        m.insert("gws", "https://www.google.com/search?q=inauthor:%22{author}%22+intitle:%22{title}%22");
        m.insert("amzn", "https://www.amazon.com/s/ref=nb_sb_noss?url=search-alias%3Dstripbooks&field-keywords={author}+{title}");
        m.insert("gimg", "https://www.google.com/images?q=%22{author}%22+%22{title}%22");
        m
    };

    static ref NAMES: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("goodreads", "Goodreads");
        m.insert("google", "Google Books");
        m.insert("wikipedia", "Wikipedia");
        m.insert("gws", "Google web search");
        m.insert("amzn", "Amazon");
        m.insert("gimg", "Google Images");
        m
    };
}

/// URL encode a string. If use_plus is true, spaces are replaced by +.
fn qquote(val: &str, use_plus: bool) -> String {
    let encoded: String = form_urlencoded::byte_serialize(val.as_bytes()).collect();
    if !use_plus {
        encoded.replace("+", "%20")
    } else {
        encoded
    }
}

fn specialized_quote(template: &str, val: &str) -> String {
    // Legacy logic: if 'goodreads.com' NOT in template, use_plus=True.
    // So if goodreads is present, use_plus=False (which seems backward compared to Python logic?
    // Python: return qquote(val, 'goodreads.com' not in template)
    // If 'goodreads.com' IS in template, 'not in' is False -> use_plus=False.
    // If 'goodreads.com' IS NOT in template, 'not in' is True -> use_plus=True.
    // Wait, let's verify Python qquote:
    // quote_plus spaces -> +
    // quote spaces -> %20
    // So goodreads uses %20 for spaces?
    // Let's stick to the logic:
    qquote(val, !template.contains("goodreads.com"))
}

pub fn url_for_author_search(key: &str, author: &str) -> Option<String> {
    if let Some(template) = AUTHOR_SEARCHES.get(key) {
        let encoded_author = specialized_quote(template, author);
        Some(template.replace("{author}", &encoded_author))
    } else {
        None
    }
}

pub fn url_for_book_search(key: &str, title: &str, author: &str) -> Option<String> {
    if let Some(template) = BOOK_SEARCHES.get(key) {
        let encoded_author = specialized_quote(template, author);
        let encoded_title = specialized_quote(template, title);
        Some(
            template
                .replace("{author}", &encoded_author)
                .replace("{title}", &encoded_title),
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_author_search() {
        let url = url_for_author_search("goodreads", "John Doe").unwrap();
        // goodreads has "goodreads.com", so use_plus=False -> %20
        assert_eq!(url, "https://www.goodreads.com/book/author/John%20Doe");

        let url = url_for_author_search("google", "John Doe").unwrap();
        // google no goodreads.com -> use_plus=True -> +
        // wait, google template: q=inauthor:%22{author}%22
        // qquote("John Doe", true) -> John+Doe
        assert_eq!(
            url,
            "https://www.google.com/search?tbm=bks&q=inauthor:%22John+Doe%22"
        );
    }

    #[test]
    fn test_book_search() {
        let url = url_for_book_search("amzn", "My Book", "John Doe").unwrap();
        // amzn -> use_plus=True
        assert!(url.contains("field-keywords=John+Doe+My+Book"));
    }
}
