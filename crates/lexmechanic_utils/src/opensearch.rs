//! Port of `old_src/src/calibre/utils/opensearch/` (issue #73):
//! parsing an [OpenSearch description
//! document](https://github.com/dewitt/opensearch) into a query
//! template, and substituting search parameters into that template.
//!
//! # Scope
//!
//! Real: [`Description::parse_xml`] (the XPath-driven document
//! parsing from `description.py`'s `load`, using `roxmltree`
//! descendant-walking in place of `lxml`'s `//*[local-name() = ...]`
//! XPath queries -- `roxmltree`'s `tag_name().name()` already strips
//! any namespace prefix, matching `local-name()`'s own behavior), the
//! v1.0/v1.1 template-selection logic (`get_url_by_type`/
//! `get_best_template`), and [`Query`]'s macro-substitution logic
//! (`query.py`, using the `url` crate's `form_urlencoded` for the
//! same percent-encoding/`+`-as-space semantics as Python's
//! `urlencode`/`parse_qs`). [`Description::load`] does a real
//! network fetch (`reqwest::blocking`, matching upstream's own
//! `browser().open(url, timeout=15)`) -- see this module's own tests
//! for how that's verified without needing real internet access (a
//! local, in-process HTTP server).

use std::collections::HashMap;

use indexmap::IndexMap;
use roxmltree::{Document, Node};

/// Port of `url.py`'s `URL` class.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Url {
    pub r#type: String,
    pub template: String,
    pub method: String,
}

impl Url {
    pub fn new(r#type: &str, template: &str) -> Self {
        Url { r#type: r#type.to_string(), template: template.to_string(), method: "GET".to_string() }
    }
}

const STANDARD_MACROS: &[&str] =
    &["searchTerms", "count", "startIndex", "startPage", "language", "outputEncoding", "inputEncoding"];

/// Port of `query.py`'s `Query` class: substitutes search parameter
/// values into a URL template's macro placeholders (e.g.
/// `q={searchTerms}`).
pub struct Query {
    base: String,
    query: IndexMap<String, Vec<String>>,
    /// Opensearch macro name (e.g. `"searchTerms"`) -> the query
    /// parameter name it was found in (e.g. `"q"`).
    macro_map: HashMap<String, String>,
}

fn split_url(format: &str) -> (&str, &str, &str) {
    let (before_fragment, fragment) = match format.find('#') {
        Some(i) => (&format[..i], &format[i..]),
        None => (format, ""),
    };
    match before_fragment.find('?') {
        Some(i) => (&before_fragment[..i], &before_fragment[i + 1..], fragment),
        None => (before_fragment, "", fragment),
    }
}

fn parse_query_string(qs: &str) -> IndexMap<String, Vec<String>> {
    let mut out: IndexMap<String, Vec<String>> = IndexMap::new();
    for (k, v) in url::form_urlencoded::parse(qs.as_bytes()) {
        // Matches Python's `parse_qs` default `keep_blank_values=False`.
        if v.is_empty() {
            continue;
        }
        out.entry(k.into_owned()).or_default().push(v.into_owned());
    }
    out
}

impl Query {
    /// `format` is a URL template obtained from an opensearch
    /// [`Description`], e.g.
    /// `"http://example.com/search?q={searchTerms}&start={startIndex}"`.
    pub fn new(format: &str) -> Self {
        let (base, qs, fragment) = split_url(format);
        let query = parse_query_string(qs);

        let mut macro_map = HashMap::new();
        for (name, values) in &query {
            let candidate = values[0].replace(['{', '}', '?'], "");
            if STANDARD_MACROS.contains(&candidate.as_str()) {
                macro_map.insert(candidate, name.clone());
            }
        }

        Query { base: format!("{base}{fragment}"), query, macro_map }
    }

    /// Substitutes `values` (opensearch macro name -> value, e.g.
    /// `"searchTerms" -> "zx81"`) into the template and returns the
    /// resulting URL. A macro present in the template but missing
    /// from `values` has its query parameter dropped entirely
    /// (matching upstream's `del query_string[name]`).
    pub fn url(&self, values: &HashMap<&str, String>) -> String {
        let mut query = self.query.clone();
        for (macro_name, param_name) in &self.macro_map {
            match values.get(macro_name.as_str()) {
                Some(v) => {
                    query.insert(param_name.clone(), vec![v.clone()]);
                }
                None => {
                    query.shift_remove(param_name);
                }
            }
        }

        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (name, vals) in &query {
            for v in vals {
                serializer.append_pair(name, v);
            }
        }
        let encoded = serializer.finish();

        let (base, _old_qs, fragment) = split_url(&self.base);
        if encoded.is_empty() {
            format!("{base}{fragment}")
        } else {
            format!("{base}?{encoded}{fragment}")
        }
    }

    pub fn has_macro(&self, r#macro: &str) -> bool {
        self.macro_map.contains_key(r#macro)
    }
}

/// Port of `description.py`'s `Description` class.
#[derive(Debug, Clone, Default)]
pub struct Description {
    /// v1.1: multiple query targets, one per MIME type.
    pub urls: Vec<Url>,
    /// v1.0: a single template, as the `<Url>` element's own text.
    pub url: String,
    pub format: String,
    pub shortname: String,
    pub longname: String,
    pub description: String,
    pub image: String,
    pub sameplesearch: String,
    pub developer: String,
    pub contact: String,
    pub attribution: String,
    pub syndicationright: String,
    pub tags: Vec<String>,
    pub adultcontent: bool,
}

fn local_name<'input>(node: &Node<'_, 'input>) -> &'input str {
    node.tag_name().name()
}

fn text_content(node: &Node<'_, '_>) -> String {
    node.descendants().filter(|n| n.is_text()).map(|n| n.text().unwrap_or("")).collect()
}

fn first_text_by_name(doc: &Document, name: &str) -> String {
    doc.root_element().descendants().find(|n| n.is_element() && local_name(n) == name).map(|n| text_content(&n)).unwrap_or_default()
}

impl Description {
    /// Port of `Description.load`'s document-parsing half (everything
    /// after the network fetch).
    pub fn parse_xml(xml: &str) -> Result<Description, roxmltree::Error> {
        let doc = Document::parse(xml)?;
        let mut d = Description::default();

        for elem in doc.root_element().descendants().filter(|n| n.is_element() && local_name(n) == "Url") {
            if let (Some(template), Some(r#type)) = (elem.attribute("template"), elem.attribute("type")) {
                d.urls.push(Url::new(r#type, template));
            }
        }
        for elem in doc.root_element().descendants().filter(|n| n.is_element() && local_name(n) == "link") {
            if elem.attribute("rel") != Some("search") {
                continue;
            }
            if let (Some(href), Some(r#type)) = (elem.attribute("href"), elem.attribute("type")) {
                d.urls.push(Url::new(r#type, href));
            }
        }

        if d.urls.is_empty() {
            d.url = first_text_by_name(&doc, "Url");
        }
        d.format = first_text_by_name(&doc, "Format");
        d.shortname = first_text_by_name(&doc, "ShortName");
        d.longname = first_text_by_name(&doc, "LongName");
        d.description = first_text_by_name(&doc, "Description");
        d.image = first_text_by_name(&doc, "Image");
        d.sameplesearch = first_text_by_name(&doc, "SampleSearch");
        d.developer = first_text_by_name(&doc, "Developer");
        d.contact = first_text_by_name(&doc, "Contact");
        d.attribution = first_text_by_name(&doc, "Attribution");
        d.syndicationright = first_text_by_name(&doc, "SyndicationRight");

        let tag_text: String = doc
            .root_element()
            .descendants()
            .filter(|n| n.is_element() && local_name(n) == "Tags")
            .map(|n| text_content(&n))
            .collect::<Vec<_>>()
            .join(" ");
        if !tag_text.is_empty() {
            d.tags = tag_text.split(' ').map(str::to_string).collect();
        }

        d.adultcontent = doc
            .root_element()
            .descendants()
            .filter(|n| n.is_element() && local_name(n) == "AdultContent")
            .any(|n| text_content(&n).contains("true"));

        Ok(d)
    }

    /// Port of `Description.load`: fetches `url` and parses the
    /// opensearch description document found there.
    pub fn load(url: &str) -> anyhow::Result<Description> {
        let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(15)).build()?;
        let body = client.get(url).send()?.error_for_status()?.text()?;
        Ok(Description::parse_xml(&body)?)
    }

    /// Port of `get_url_by_type`.
    pub fn get_url_by_type(&self, r#type: &str) -> Option<&Url> {
        self.urls.iter().find(|u| u.r#type == r#type)
    }

    /// Port of `get_best_template`.
    pub fn get_best_template(&self) -> Option<&str> {
        if !self.url.is_empty() {
            return Some(&self.url);
        }
        for candidate in ["application/atom+xml", "application/rss+xml", "text/xml"] {
            if let Some(u) = self.get_url_by_type(candidate) {
                return Some(&u.template);
            }
        }
        self.urls.first().map(|u| u.template.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1_1_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Example</ShortName>
  <LongName>Example Search</LongName>
  <Description>Search Example.com</Description>
  <Tags>example web search</Tags>
  <Contact>admin@example.com</Contact>
  <Url type="application/atom+xml" template="http://example.com/search?q={searchTerms}&amp;start={startIndex?}"/>
  <Url type="application/rss+xml" template="http://example.com/search.rss?q={searchTerms}"/>
  <Image>http://example.com/icon.png</Image>
  <AdultContent>false</AdultContent>
</OpenSearchDescription>"#;

    const V1_0_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.0/">
  <ShortName>Old</ShortName>
  <Description>Old-style search</Description>
  <Url>http://example.com/search?q={searchTerms}</Url>
  <Format>http://a9.com/-/spec/opensearch/1.0/</Format>
</OpenSearchDescription>"#;

    #[test]
    fn parses_a_v1_1_document_with_multiple_url_templates() {
        let d = Description::parse_xml(V1_1_XML).unwrap();
        assert_eq!(d.shortname, "Example");
        assert_eq!(d.longname, "Example Search");
        assert_eq!(d.tags, vec!["example", "web", "search"]);
        assert_eq!(d.urls.len(), 2);
        assert!(!d.adultcontent);
        assert_eq!(d.get_url_by_type("application/rss+xml").unwrap().template, "http://example.com/search.rss?q={searchTerms}");
    }

    #[test]
    fn best_template_prefers_atom_then_rss() {
        let d = Description::parse_xml(V1_1_XML).unwrap();
        assert_eq!(d.get_best_template(), Some("http://example.com/search?q={searchTerms}&start={startIndex?}"));
    }

    #[test]
    fn falls_back_to_the_v1_0_text_url_when_there_are_no_url_elements_with_templates() {
        let d = Description::parse_xml(V1_0_XML).unwrap();
        assert!(d.urls.is_empty());
        assert_eq!(d.url, "http://example.com/search?q={searchTerms}");
        assert_eq!(d.get_best_template(), Some("http://example.com/search?q={searchTerms}"));
    }

    #[test]
    fn substitutes_macros_and_drops_missing_ones() {
        let q = Query::new("http://example.com/search?q={searchTerms}&start={startIndex?}&fixed=1");
        assert!(q.has_macro("searchTerms"));
        assert!(q.has_macro("startIndex"));
        assert!(!q.has_macro("count"));

        let mut values = HashMap::new();
        values.insert("searchTerms", "zx81".to_string());
        let url = q.url(&values);
        assert!(url.contains("q=zx81"), "{url}");
        assert!(!url.contains("start="), "{url} should have dropped the unfilled startIndex macro");
        assert!(url.contains("fixed=1"), "{url} should keep non-macro params untouched");
    }

    #[test]
    fn substitutes_multiple_macros() {
        let q = Query::new("http://example.com/search?q={searchTerms}&start={startIndex}&n={count}");
        let mut values = HashMap::new();
        values.insert("searchTerms", "rust".to_string());
        values.insert("startIndex", "1".to_string());
        values.insert("count", "25".to_string());
        let url = q.url(&values);
        assert!(url.contains("q=rust"));
        assert!(url.contains("start=1"));
        assert!(url.contains("n=25"));
    }

    #[test]
    fn load_fetches_and_parses_a_real_http_response() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = V1_1_XML.as_bytes();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
            stream.flush().unwrap();
        });

        let d = Description::load(&format!("http://127.0.0.1:{port}/description.xml")).unwrap();
        handle.join().unwrap();
        assert_eq!(d.shortname, "Example");
        assert_eq!(d.urls.len(), 2);
    }
}
