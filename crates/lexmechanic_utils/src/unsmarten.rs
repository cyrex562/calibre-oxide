use crate::mreplace::MReplace;
use lazy_static::lazy_static;
use std::collections::HashMap;

lazy_static! {
    static ref REPLACER: MReplace = {
        let mut m = HashMap::new();
        m.insert("&#8211;", "--");
        m.insert("&ndash;", "--");
        m.insert("–", "--");
        m.insert("&#8212;", "---");
        m.insert("&mdash;", "---");
        m.insert("—", "---");
        m.insert("&#8230;", "...");
        m.insert("&hellip;", "...");
        m.insert("…", "...");
        m.insert("&#8220;", "\"");
        m.insert("&#8221;", "\"");
        m.insert("&#8222;", "\"");
        m.insert("&#8243;", "\"");
        m.insert("&ldquo;", "\"");
        m.insert("&rdquo;", "\"");
        m.insert("&bdquo;", "\"");
        m.insert("&Prime;", "\"");
        m.insert("“", "\"");
        m.insert("”", "\"");
        m.insert("„", "\"");
        m.insert("″", "\"");
        m.insert("&#8216;", "'");
        m.insert("&#8217;", "'");
        m.insert("&#8242;", "'");
        m.insert("&lsquo;", "'");
        m.insert("&rsquo;", "'");
        m.insert("&prime;", "'");
        m.insert("‘", "'");
        m.insert("’", "'");
        m.insert("′", "'");
        MReplace::new(&m)
    };
}

pub fn unsmarten_text(text: &str) -> String {
    REPLACER.replace(text)
}
