//! Port of `calibre.utils.ffml_processor` (issue #521): a small parser
//! plus three renderers (HTML, RST, "transifex") for FFML (Formatter
//! Function Markup Language), the docstring-only markup calibre's
//! built-in template functions use for their own documentation/tooltip
//! text. Unrelated to template *evaluation* -- see
//! [`crate::formatter`] for that.
//!
//! # Disclosed narrowings
//!
//! - `prepare_string_for_xml`'s full HTML5 named-entity decode table
//!   lives in `calibre_ebooks::html_entities`, unreachable from this
//!   crate (`calibre_utils` sits *below* `calibre_ebooks` in the
//!   dependency graph, never the reverse). [`escape_xml`] here only
//!   undoes the 5 XML-predefined entities before re-escaping, rather
//!   than the full named-entity table. Every real FFML source in this
//!   codebase is a hand-authored plain-text docstring with no HTML
//!   entities in it, so this has no practical effect on real callers.
//! - Real `parse_document`'s `safe=true` recovery path can fall back to
//!   a translated-English document via a `formatted_english` attribute
//!   on the input, a GUI-only localization mechanism
//!   (`calibre.utils.localization`) not modeled here. [`parse_document`]
//!   takes a plain `&str`, so it always takes the "no English fallback
//!   available" branch on a parse error -- an error-annotated document
//!   containing the raw offending text. This matches real behavior for
//!   every actual call site in this codebase (there is no GUI
//!   translation layer here yet).
//! - No GUI tooltip surface calls this module anywhere in this port yet
//!   (matching the issue's own "low priority" framing) -- ported and
//!   tested as a pure library, not wired into anything.
//! - `document_to_summary_html`/`document_to_summary_rst`'s function-name
//!   extraction (`document[0:document.find('(')]`) assumes a `(` is
//!   always present, true for every real FFML function-doc signature.
//!   Malformed input without a `(` is treated as "no function name
//!   found" here rather than reproducing Python's negative-index slice
//!   quirk for that case.

use std::fmt::Write as _;

/// `MARKUP_ERROR` in `ffml_processor.py` (the English literal -- real
/// Python translates it via `_()`, unreachable here without a GUI
/// translation layer).
pub const MARKUP_ERROR_PREFIX: &str = "*Template documentation markup error*:";

/// A parsed FFML node. Flattens Python's `Node` class hierarchy (one
/// subclass per `NodeKinds` variant, most just wrapping a `_text`
/// field) into a single enum, matching this port's established pattern
/// for small closed-node-kind trees (e.g. `calibre_utils::formatter::ast::ExprKind`).
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Document(Vec<Node>),
    BlankLine,
    BoldText(String),
    /// A single `\`-escaped character. Kept as a `String` (not `char`)
    /// to mirror Python's own `CharacterNode(character: str)`.
    Character(String),
    CodeText(String),
    CodeBlock(String),
    /// Internal-only in real Python too ("no FFML support to generate
    /// this node" -- only [`parse_document`]'s own error-recovery path
    /// creates one).
    ErrorText(String),
    GuiLabel(String),
    ItalicText(String),
    List(Vec<Node>),
    ListItem(Vec<Node>),
    Ref(String),
    EndSummary,
    Text(String),
    Url { label: String, url: String },
}

/// Port of `prepare_string_for_xml(raw, attribute=False)`. See the
/// module doc's disclosed narrowing on entity decoding.
pub fn escape_xml(raw: &str) -> String {
    let decoded = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    decoded.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A real FFML syntax error. Port of the `ValueError` real `self.error`
/// raises, carrying the same `"{message} on line {line} in \"{name}\""`
/// formatting.
#[derive(Debug, Clone)]
pub struct FfmlError {
    pub message: String,
    pub line: u32,
    pub document_name: String,
}

impl std::fmt::Display for FfmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} on line {} in \"{}\"", self.message, self.line, self.document_name)
    }
}

impl std::error::Error for FfmlError {}

pub type FfmlResult<T> = Result<T, FfmlError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keyword {
    CodeText,
    ItalicText,
    BoldText,
    CodeBlock,
    EndSummary,
    GuiLabel,
    List,
    EndList,
    Ref,
    Url,
    ListItem,
    BlankLine,
    Character,
}

/// Port of the `keywords` dict. Order matters: `` ` `` must come after
/// `` `` `` (checked first), matching the real dict's own ordering and
/// its comment ("must be before '`'").
const KEYWORDS: &[(&str, Keyword)] = &[
    ("``", Keyword::CodeText),
    ("`", Keyword::ItalicText),
    ("[B]", Keyword::BoldText),
    ("[CODE]", Keyword::CodeBlock),
    ("[/]", Keyword::EndSummary),
    (":guilabel:", Keyword::GuiLabel),
    ("[LIST]", Keyword::List),
    ("[/LIST]", Keyword::EndList),
    (":ref:", Keyword::Ref),
    ("[URL", Keyword::Url),
    ("[*]", Keyword::ListItem),
    ("\n\n", Keyword::BlankLine),
    ("\\", Keyword::Character),
];

/// Port of `can_be_inlined`.
const CAN_BE_INLINED: &[Keyword] = &[
    Keyword::CodeText,
    Keyword::ItalicText,
    Keyword::BoldText,
    Keyword::EndSummary,
    Keyword::GuiLabel,
    Keyword::Ref,
    Keyword::Url,
    Keyword::Character,
];

enum FindResult {
    /// Byte distance to the next keyword occurrence (or to the end of
    /// the input if none remain) -- always `> 0` when returned from
    /// [`Parser::find_one_of_at`], since a distance of exactly `0`
    /// always short-circuits to [`FindResult::Keyword`] instead.
    Distance(usize),
    Keyword(Keyword),
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    line: u32,
    document_name: String,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, document_name: &str) -> Self {
        Parser { input, pos: 0, line: 1, document_name: document_name.to_string() }
    }

    fn error<T>(&self, message: impl Into<String>) -> FfmlResult<T> {
        Err(FfmlError { message: message.into(), line: self.line, document_name: self.document_name.clone() })
    }

    fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    /// Port of `find`: byte distance from `at` to `needle`'s next
    /// occurrence at or after `at`, or `None` if absent. Rust's
    /// byte-offset `str::find` plays the same role as Python's
    /// character-offset `str.find` here: since every `needle` is ASCII,
    /// the two indexing schemes always agree on which bytes/characters
    /// make up the resulting substrings, even though the raw numbers
    /// they produce differ whenever non-ASCII text precedes a match.
    fn find_from(&self, needle: &str, at: usize) -> Option<usize> {
        self.input[at..].find(needle)
    }

    fn find(&self, needle: &str) -> Option<usize> {
        self.find_from(needle, self.pos)
    }

    /// Port of `move_pos`. `n` is a byte count; every real caller
    /// derives `n` from either an ASCII keyword's own length or a
    /// [`Self::find`] result (always a valid `str` boundary), so this
    /// never lands mid-character. Clamped to `input.len()` -- real
    /// Python's own `move_pos` can push `input_pos` past `len(input)`
    /// when `find_one_of`'s "no keyword found" fallback (`len(self.input)`,
    /// not `len(self.input) - pos`) is used, but that overshoot has no
    /// observable effect there either (Python slicing/`at_end()` both
    /// tolerate or clamp it) -- clamping here reproduces the same real
    /// behavior without needing `usize` saturating arithmetic at every
    /// call site.
    fn move_pos(&mut self, n: usize) {
        let end = (self.pos + n).min(self.input.len());
        self.line += self.input[self.pos..end].matches('\n').count() as u32;
        self.pos = end;
    }

    fn text_to(&self, len: usize) -> &'a str {
        &self.input[self.pos..(self.pos + len).min(self.input.len())]
    }

    fn text_to_no_newline(&self, len: usize, block_name: &str) -> FfmlResult<&'a str> {
        let txt = self.text_to(len);
        if txt.contains('\n') {
            return self.error(format!("Newline unexpected in {block_name}"));
        }
        Ok(txt)
    }

    fn starts_with(&self, s: &str) -> bool {
        self.input[self.pos..].starts_with(s)
    }

    /// A single raw-byte `text_to(1) == '\n'` check in real Python is
    /// unsafe to port literally (a 1-*byte* slice can split a
    /// multi-byte character and panic in Rust) -- ported as a
    /// `starts_with` check instead, which is exactly what the Python
    /// comparison means in practice (`'\n'` is always 1 byte).
    fn peek_is_newline(&self) -> bool {
        self.input[self.pos..].starts_with('\n')
    }

    fn find_one_of_at(&self, at: usize) -> FindResult {
        let mut best: Option<usize> = None;
        for (kw, kind) in KEYWORDS {
            if let Some(d) = self.find_from(kw, at) {
                if d == 0 {
                    return FindResult::Keyword(*kind);
                }
                best = Some(best.map_or(d, |b: usize| b.min(d)));
            }
        }
        FindResult::Distance(best.unwrap_or(self.input.len() - at))
    }

    fn find_one_of(&self) -> FindResult {
        self.find_one_of_at(self.pos)
    }

    fn get_bold_text(&mut self) -> FfmlResult<Node> {
        self.move_pos("[B]".len());
        let Some(end) = self.find("[/B]") else {
            return self.error("Missing closing \"[/B]\" for bold");
        };
        let node = Node::BoldText(self.text_to(end).to_string());
        self.move_pos(end + "[/B]".len());
        Ok(node)
    }

    /// Port of `get_character`: reads one `\`-escaped Unicode character
    /// (not necessarily 1 byte -- real Python's own char-indexed
    /// `text_to(1)` handles this for free; ported explicitly here via
    /// `chars().next()` rather than the generic byte-oriented
    /// `text_to`/`move_pos`).
    fn get_character(&mut self) -> Node {
        self.move_pos(1);
        let ch = self.input[self.pos..].chars().next().unwrap_or('\0');
        self.move_pos(ch.len_utf8());
        Node::Character(ch.to_string())
    }

    fn get_code_block(&mut self) -> FfmlResult<Node> {
        self.move_pos("[CODE]".len());
        if self.peek_is_newline() {
            self.move_pos(1);
        }
        let Some(end) = self.find("[/CODE]") else {
            return self.error("Missing [/CODE] for block");
        };
        let text = self.text_to(end).replace(r"[\/CODE]", "[/CODE]");
        self.move_pos(end + "[/CODE]".len());
        if self.peek_is_newline() {
            self.move_pos(1);
        }
        Ok(Node::CodeBlock(text))
    }

    fn get_code_text(&mut self) -> FfmlResult<Node> {
        self.move_pos("``".len());
        let Some(end) = self.find("``") else {
            return self.error("Missing closing \"``\" for CODE_TEXT");
        };
        let text = self.text_to(end).trim_end_matches(' ').to_string();
        self.move_pos(end + "``".len());
        Ok(Node::CodeText(text))
    }

    fn get_gui_label(&mut self) -> FfmlResult<Node> {
        self.move_pos(":guilabel:`".len());
        let Some(end) = self.find("`") else {
            return self.error("Missing ` (backquote) for :guilabel:");
        };
        let text = self.text_to_no_newline(end, "GUI_LABEL (:guilabel:`)")?.to_string();
        self.move_pos(end + "`".len());
        Ok(Node::GuiLabel(text))
    }

    fn get_italic_text(&mut self) -> FfmlResult<Node> {
        self.move_pos(1);
        let Some(end) = self.find("`") else {
            return self.error("Missing closing \"`\" for italics");
        };
        let node = Node::ItalicText(self.text_to(end).to_string());
        self.move_pos(end + 1);
        Ok(node)
    }

    fn get_list(&mut self) -> FfmlResult<Node> {
        self.move_pos("[LIST]\n".len());
        let mut items = Vec::new();
        loop {
            if self.starts_with("[/LIST]") {
                break;
            }
            if !self.starts_with("[*]") {
                return self.error(format!("Missing [*] in list near text:\"{}\"", self.text_to(10)));
            }
            self.move_pos("[*]".len());
            let mut children = Vec::new();
            self.parse_children(&mut children)?;
            items.push(Node::ListItem(children));
        }
        self.move_pos("[/LIST]".len());
        if self.peek_is_newline() {
            self.move_pos(1);
        }
        Ok(Node::List(items))
    }

    fn get_ref(&mut self) -> FfmlResult<Node> {
        self.move_pos(":ref:`".len());
        let Some(end) = self.find("`") else {
            return self.error("Missing ` (backquote) for :ref:");
        };
        let text = self.text_to_no_newline(end, "REF (:ref:`)")?.to_string();
        self.move_pos(end + "`".len());
        Ok(Node::Ref(text))
    }

    fn get_url(&mut self) -> FfmlResult<Node> {
        self.move_pos("[URL".len());
        let Some(hp) = self.find("href=\"") else {
            return self.error(format!("Missing href=\" near text {}", self.text_to(10)));
        };
        self.move_pos(hp + "href=\"".len());
        let Some(close_quote) = self.find("\"]") else {
            return self.error(format!("Missing closing \"> for URL near text:\"{}\"", self.text_to(10)));
        };
        let href = self.text_to_no_newline(close_quote, "URL href")?.to_string();
        self.move_pos(close_quote + "\"]".len());
        let Some(lp) = self.find("[/URL]") else {
            return self.error(format!("Missing closing [/URL] near text {}", self.text_to(10)));
        };
        let label = self.text_to(lp).trim().replace('\n', " ");
        self.move_pos(lp + "[/URL]".len());
        Ok(Node::Url { label, url: href })
    }

    /// Port of `_parse_document`. Appends parsed nodes to `children`
    /// (the caller wraps them in `Node::Document`/`Node::ListItem`);
    /// returns early, leaving `self.pos` unmoved, on a `[*]`/`[/LIST]`
    /// marker so the caller ([`Self::get_list`]) can consume it itself.
    fn parse_children(&mut self, children: &mut Vec<Node>) -> FfmlResult<()> {
        loop {
            match self.find_one_of() {
                FindResult::Distance(d) => {
                    let txt = self.text_to(d);
                    if txt != "\n" {
                        let last_char = txt.chars().next_back().expect("d > 0 implies non-empty txt");
                        let butlast = &txt[..txt.len() - last_char.len_utf8()];
                        let mut new_txt = butlast.replace('\n', " ");
                        let mut last = last_char;
                        if last_char == '\n' {
                            if let FindResult::Keyword(k) = self.find_one_of_at(self.pos + d) {
                                if CAN_BE_INLINED.contains(&k) {
                                    last = ' ';
                                }
                            }
                        }
                        new_txt.push(last);
                        children.push(Node::Text(new_txt));
                    } else {
                        children.push(Node::Text(txt.to_string()));
                    }
                    self.move_pos(d);
                }
                FindResult::Keyword(Keyword::BlankLine) => {
                    children.push(Node::BlankLine);
                    self.move_pos(2);
                }
                FindResult::Keyword(Keyword::BoldText) => children.push(self.get_bold_text()?),
                FindResult::Keyword(Keyword::Character) => children.push(self.get_character()),
                FindResult::Keyword(Keyword::CodeText) => children.push(self.get_code_text()?),
                FindResult::Keyword(Keyword::CodeBlock) => children.push(self.get_code_block()?),
                FindResult::Keyword(Keyword::EndSummary) => {
                    children.push(Node::EndSummary);
                    self.move_pos(3);
                }
                FindResult::Keyword(Keyword::GuiLabel) => children.push(self.get_gui_label()?),
                FindResult::Keyword(Keyword::ItalicText) => children.push(self.get_italic_text()?),
                FindResult::Keyword(Keyword::List) => children.push(self.get_list()?),
                FindResult::Keyword(Keyword::ListItem) => return Ok(()),
                FindResult::Keyword(Keyword::EndList) => return Ok(()),
                FindResult::Keyword(Keyword::Ref) => children.push(self.get_ref()?),
                FindResult::Keyword(Keyword::Url) => children.push(self.get_url()?),
            }
            if self.at_end() {
                break;
            }
        }
        Ok(())
    }
}

/// Port of `parse_document(doc, name, safe=False)`'s underlying
/// `self._parse_document(node)` call: parses `doc` and propagates a
/// real syntax error instead of recovering from it. Every real caller
/// in this codebase uses the `safe=true` default -- see
/// [`parse_document`] -- except real upstream's one live-editor preview
/// dialog, which doesn't exist in this port; kept for API completeness
/// and because it's what `safe=true` recovery is built on top of.
pub fn try_parse_document(doc: &str, name: &str) -> FfmlResult<Node> {
    if doc.is_empty() {
        return Ok(Node::Document(Vec::new()));
    }
    let mut parser = Parser::new(doc, name);
    let mut children = Vec::new();
    parser.parse_children(&mut children)?;
    Ok(Node::Document(children))
}

/// Port of `parse_document(doc, name, safe=True)` (the default, and the
/// only mode every real call site in this codebase uses). On a parse
/// error, returns a document containing the error message and the raw
/// offending text -- see the module doc's disclosed narrowing on the
/// `formatted_english` GUI-fallback branch this collapses.
pub fn parse_document(doc: &str, name: &str) -> Node {
    match try_parse_document(doc, name) {
        Ok(tree) => tree,
        Err(e) => Node::Document(vec![
            Node::ErrorText(MARKUP_ERROR_PREFIX.to_string()),
            Node::Text(format!(" {e}")),
            Node::BlankLine,
            Node::ErrorText("Documentation containing the error:".to_string()),
            Node::Text(doc.to_string()),
        ]),
    }
}

fn escaped(text: &str) -> String {
    escape_xml(text)
}

/// Port of `tree_to_html`. `depth` is dropped from the signature: real
/// Python's own parameter is threaded through recursive calls but never
/// read for any decision, so it has no observable effect.
pub fn tree_to_html(tree: &Node) -> String {
    let mut result = String::new();
    match tree {
        Node::Text(t) => result.push_str(&escaped(t)),
        Node::BoldText(t) => {
            let _ = write!(result, "<b>{}</b>", escaped(t));
        }
        Node::BlankLine => result.push_str("\n<br>\n<br>\n"),
        Node::Character(t) => result.push_str(t),
        Node::CodeText(t) => {
            let _ = write!(result, "<code>{}</code>", escaped(t));
        }
        Node::CodeBlock(t) => {
            let _ = write!(result, "<pre style=\"margin-left:2em\"><code>{}</code></pre>", escaped(t).trim_end());
        }
        Node::EndSummary => {}
        Node::ErrorText(t) => {
            let _ = write!(result, "<span style=\"color:red\"><strong>{}</strong></span>", escaped(t));
        }
        Node::GuiLabel(t) => {
            let _ = write!(result, "<span style=\"font-family: Sans-Serif\">{}</span>", escaped(t));
        }
        Node::ItalicText(t) => {
            let _ = write!(result, "<i>{}</i>", escaped(t));
        }
        Node::List(items) => {
            result.push_str("<ul>\n");
            for child in items {
                result.push_str("<li>\n");
                result.push_str(&tree_to_html(child));
                result.push_str("</li>\n");
            }
            result.push_str("</ul>\n");
        }
        Node::Ref(t) => {
            let _ = write!(result, "<a href=\"ffdoc:{t}\">{t}</a></a>");
        }
        Node::Url { label, url } => {
            let _ = write!(result, "<a href=\"{}\">{}</a>", escaped(url), escaped(label));
        }
        Node::Document(children) | Node::ListItem(children) => {
            for child in children {
                result.push_str(&tree_to_html(child));
            }
        }
    }
    result
}

/// Port of `document_to_html`.
pub fn document_to_html(document: &str, name: &str) -> String {
    tree_to_html(&parse_document(document, name))
}

/// Port of `document_to_summary_html`.
pub fn document_to_summary_html(document: &str, name: &str) -> String {
    let document = document.trim();
    let document = match document.find("[/]") {
        Some(sum_tag) if sum_tag > 0 => &document[..sum_tag],
        _ => document,
    };
    let fname = match document.find('(') {
        Some(p) => document[..p].trim_start_matches('`'),
        None => document.trim_start_matches('`'),
    };
    let tree = parse_document(document, name);
    let result = tree_to_html(&tree);
    let paren = result.find('(').unwrap_or(0);
    format!("<a href=\"ffdoc:{fname}\">{fname}</a>{}", &result[paren..])
}

/// Port of `tree_to_transifex`. `depth` dropped -- see [`tree_to_html`]'s
/// doc for why.
pub fn tree_to_transifex(tree: &Node) -> String {
    let mut result = String::new();
    match tree {
        Node::Text(t) => result.push_str(t),
        Node::BoldText(t) => {
            let _ = write!(result, "[B]{t}[/B]");
        }
        Node::BlankLine => result.push_str("\n\n"),
        Node::Character(t) => {
            result.push('\\');
            result.push_str(t);
        }
        Node::CodeText(t) => {
            let t = if t.ends_with('`') { format!("{t} ") } else { t.clone() };
            let _ = write!(result, "``{t}``");
        }
        Node::CodeBlock(t) => {
            let _ = write!(result, "[CODE]\n{}[/CODE]\n", t.replace("[/CODE]", r"[\/CODE]"));
        }
        Node::EndSummary => result.push_str("[/]"),
        Node::ErrorText(t) => result.push_str(t),
        Node::GuiLabel(t) => {
            let _ = write!(result, ":guilabel:`{t}`");
        }
        Node::ItalicText(t) => {
            let _ = write!(result, "`{t}`");
        }
        Node::List(items) => {
            result.push_str("[LIST]\n");
            for child in items {
                result.push_str("[*]");
                let t = tree_to_transifex(child);
                result.push_str(t.strip_suffix('\n').unwrap_or(&t));
            }
            result.push_str("[/LIST]\n");
        }
        Node::Ref(t) => {
            let _ = write!(result, ":ref:`{t}`");
        }
        Node::Url { label, url } => {
            let _ = write!(result, "[URL href=\"{url}\"]{label}[/URL]");
        }
        Node::ListItem(children) => {
            for child in children {
                result.push_str(&tree_to_transifex(child));
            }
            result.push('\n');
        }
        Node::Document(children) => {
            for child in children {
                result.push_str(&tree_to_transifex(child));
            }
        }
    }
    result
}

/// Port of `document_to_transifex`.
pub fn document_to_transifex(document: &str, name: &str) -> String {
    tree_to_transifex(&parse_document(document, name))
}

fn indent_text(result: &mut String, indent: usize, txt: &str) {
    let indent_str = "  ".repeat(indent);
    if result.is_empty() {
        result.push_str(&txt.trim_start().replace('\n', &indent_str));
    } else if result.ends_with('\n') {
        result.push_str(&indent_str);
        result.push_str(&txt.trim_start().replace('\n', &indent_str));
    } else {
        result.push_str(&txt.replace('\n', &indent_str));
    }
}

fn tree_to_rst_into(tree: &Node, indent: usize, result: &mut String) {
    match tree {
        Node::BlankLine => result.push_str("\n\n"),
        Node::BoldText(t) => {
            let prefix = if result.ends_with('?') { "\\ " } else { "" };
            indent_text(result, indent, &format!("{prefix}**{t}**"));
        }
        Node::Character(t) => result.push_str(t),
        Node::CodeBlock(t) => {
            let ind = "  ".repeat(indent);
            let _ = write!(result, "\n\n{ind}::\n\n");
            for line in t.trim().split('\n') {
                let _ = writeln!(result, "{}{line}", "  ".repeat(indent + 1));
            }
            result.push('\n');
        }
        Node::CodeText(t) => indent_text(result, indent, &format!("``{t}``")),
        Node::EndSummary => {}
        Node::ErrorText(t) => indent_text(result, indent, &format!("**{t}**")),
        Node::GuiLabel(t) => indent_text(result, indent, &format!(":guilabel:`{t}`")),
        Node::ItalicText(t) => indent_text(result, indent, &format!("`{t}`")),
        Node::List(items) => {
            result.push_str("\n\n");
            for child in items {
                result.push_str(&"  ".repeat(indent));
                result.push_str("* ");
                tree_to_rst_into(child, indent + 1, result);
                result.push('\n');
            }
            result.push('\n');
        }
        Node::Ref(t) => {
            let rname = t.strip_suffix("()").unwrap_or(t);
            indent_text(result, indent, &format!(":ref:`{rname}() <ff_{rname}>`"));
        }
        Node::Text(t) => indent_text(result, indent, t),
        Node::Url { label, url } => indent_text(result, indent, &format!("`{label} <{url}>`_")),
        Node::Document(children) | Node::ListItem(children) => {
            for child in children {
                tree_to_rst_into(child, indent, result);
            }
        }
    }
}

/// Port of `tree_to_rst`.
pub fn tree_to_rst(tree: &Node, indent: usize) -> String {
    let mut result = "  ".repeat(indent);
    tree_to_rst_into(tree, indent, &mut result);
    result
}

/// `str.lstrip(chars)` treats its argument as a *character set*, not a
/// prefix string -- `doc.lstrip('  ' * indent)` in real Python strips
/// every leading space when `indent > 0` (the set is just `{' '}`
/// regardless of how many times it's repeated) and strips nothing at
/// all when `indent == 0` (an empty character set). Ported literally
/// rather than assumed to mean "strip this many leading spaces".
fn lstrip_like_python_char_set(s: &str, indent: usize) -> &str {
    if indent == 0 {
        s
    } else {
        s.trim_start_matches(' ')
    }
}

/// Port of `document_to_rst`.
pub fn document_to_rst(document: &str, name: &str, indent: usize, prefix: Option<&str>) -> String {
    let doc = tree_to_rst(&parse_document(document, name), indent);
    match prefix {
        Some(p) => format!("{p}{}", lstrip_like_python_char_set(&doc, indent)),
        None => doc,
    }
}

/// Port of `document_to_summary_rst`.
pub fn document_to_summary_rst(document: &str, name: &str, indent: usize, prefix: Option<&str>) -> String {
    let document = document.trim();
    let document = match document.find("[/]") {
        Some(sum_tag) if sum_tag > 0 => &document[..sum_tag],
        _ => document,
    };
    let fname = match document.find('(') {
        Some(p) => document[..p].trim_start_matches('`'),
        None => document.trim_start_matches('`'),
    };
    let doc = tree_to_rst(&parse_document(document, name), indent);
    let lparen = doc.find('(').unwrap_or(0);
    let doc = format!(":ref:`ff_{fname}`\\ ``{}", &doc[lparen..]);
    match prefix {
        Some(p) => format!("{p}{}", lstrip_like_python_char_set(&doc, indent)),
        None => doc,
    }
}

/// Port of `print_node_tree`, for debugging FFML parse trees.
pub fn print_node_tree(node: &Node, indent: usize) {
    let pad = " ".repeat(indent);
    match node {
        Node::Text(t) | Node::CodeText(t) | Node::Character(t) | Node::CodeBlock(t) | Node::ItalicText(t) | Node::GuiLabel(t) | Node::BoldText(t) | Node::ErrorText(t) => {
            println!("{pad}{node_name}:{t}", node_name = node_kind_name(node));
        }
        Node::Url { label, url } => println!("{pad}URL: label={label}, URL={url}"),
        _ => println!("{pad}{}", node_kind_name(node)),
    }
    let children: &[Node] = match node {
        Node::Document(c) | Node::ListItem(c) | Node::List(c) => c,
        _ => &[],
    };
    for child in children {
        print_node_tree(child, indent + 1);
    }
}

fn node_kind_name(node: &Node) -> &'static str {
    match node {
        Node::Document(_) => "DOCUMENT",
        Node::BlankLine => "BLANK_LINE",
        Node::BoldText(_) => "BOLD_TEXT",
        Node::Character(_) => "CHARACTER",
        Node::CodeText(_) => "CODE_TEXT",
        Node::CodeBlock(_) => "CODE_BLOCK",
        Node::ErrorText(_) => "ERROR_TEXT",
        Node::GuiLabel(_) => "GUI_LABEL",
        Node::ItalicText(_) => "ITALIC_TEXT",
        Node::List(_) => "LIST",
        Node::ListItem(_) => "LIST_ITEM",
        Node::Ref(_) => "REF",
        Node::EndSummary => "END_SUMMARY",
        Node::Text(_) => "TEXT",
        Node::Url { .. } => "URL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_round_trips_to_html_and_rst() {
        let tree = parse_document("Hello world", "test");
        assert_eq!(tree_to_html(&tree), "Hello world");
        assert_eq!(tree_to_rst(&tree, 0), "Hello world");
    }

    #[test]
    fn bold_italic_and_code_render_correctly() {
        let tree = parse_document("[B]bold[/B] `italic` ``code``", "test");
        assert_eq!(tree_to_html(&tree), "<b>bold</b> <i>italic</i> <code>code</code>");
        assert_eq!(tree_to_rst(&tree, 0), "**bold** `italic` ``code``");
    }

    #[test]
    fn a_blank_line_separates_paragraphs() {
        let tree = parse_document("First\n\nSecond", "test");
        assert_eq!(tree_to_html(&tree), "First\n<br>\n<br>\nSecond");
    }

    #[test]
    fn escaping_applies_to_text_but_not_to_escaped_characters() {
        let tree = parse_document(r"AT&T \& co", "test");
        let html = tree_to_html(&tree);
        assert!(html.contains("AT&amp;T"), "{html}");
        // The `\&` escape emits the raw character, unescaped, matching
        // real upstream's own (perhaps surprising) behavior.
        assert!(html.contains("& co") && !html.contains("&amp; co"), "{html}");
    }

    #[test]
    fn a_url_node_renders_a_real_link_in_html_and_rst() {
        let tree = parse_document(r#"[URL href="https://example.com"]Example[/URL]"#, "test");
        assert_eq!(tree_to_html(&tree), "<a href=\"https://example.com\">Example</a>");
        assert_eq!(tree_to_rst(&tree, 0), "`Example <https://example.com>`_");
    }

    #[test]
    fn a_ref_node_becomes_a_real_sphinx_ref_in_rst() {
        let tree = parse_document(":ref:`some_function()`", "test");
        assert_eq!(tree_to_rst(&tree, 0), ":ref:`some_function() <ff_some_function>`");
    }

    #[test]
    fn a_gui_label_and_code_block_render_in_all_three_formats() {
        let tree = parse_document(":guilabel:`OK`\n[CODE]\nlet x = 1;\n[/CODE]\n", "test");
        let html = tree_to_html(&tree);
        assert!(html.contains("<span style=\"font-family: Sans-Serif\">OK</span>"), "{html}");
        assert!(html.contains("<pre"), "{html}");
        let rst = tree_to_rst(&tree, 0);
        assert!(rst.contains(":guilabel:`OK`"), "{rst}");
        assert!(rst.contains("::\n\n"), "{rst}");
        let tf = tree_to_transifex(&tree);
        assert!(tf.contains(":guilabel:`OK`"), "{tf}");
        assert!(tf.contains("[CODE]\nlet x = 1;\n[/CODE]"), "{tf}");
    }

    #[test]
    fn a_list_renders_as_real_bullets_in_html_and_rst() {
        let tree = parse_document("[LIST]\n[*]one\n[*]two\n[/LIST]\n", "test");
        let html = tree_to_html(&tree);
        // The trailing newline before each `[*]`/`[/LIST]` marker is
        // preserved literally -- `ListItem`/`EndList` aren't in
        // `CAN_BE_INLINED`, so the text run's own newline stays put
        // rather than collapsing to a space, matching real upstream.
        assert!(html.contains("<ul>\n<li>\none\n</li>\n<li>\ntwo\n</li>\n</ul>\n"), "{html}");
        let rst = tree_to_rst(&tree, 0);
        assert!(rst.contains("* one"), "{rst}");
        assert!(rst.contains("* two"), "{rst}");
    }

    #[test]
    fn transifex_round_trips_through_the_parser_again() {
        let original = "[B]bold[/B] and `italic` with a [LIST]\n[*]one\n[*]two\n[/LIST]\n";
        let tree = parse_document(original, "test");
        let tf = document_to_transifex(original, "test");
        let tree2 = parse_document(&tf, "test-round-trip");
        assert_eq!(tree_to_html(&tree), tree_to_html(&tree2));
    }

    #[test]
    fn a_missing_closing_marker_is_a_real_reported_error_not_a_panic() {
        let err = try_parse_document("[B]unterminated bold", "test").unwrap_err();
        assert!(err.to_string().contains("Missing closing"), "{err}");
        assert!(err.to_string().contains("test"), "{err}");
    }

    #[test]
    fn safe_mode_recovers_with_an_error_annotated_document() {
        let tree = parse_document("[B]unterminated bold", "test");
        let html = tree_to_html(&tree);
        assert!(html.contains("Template documentation markup error"), "{html}");
        assert!(html.contains("unterminated bold"), "{html}");
    }

    #[test]
    fn an_empty_document_parses_to_an_empty_tree() {
        let tree = parse_document("", "test");
        assert_eq!(tree, Node::Document(Vec::new()));
        assert_eq!(tree_to_html(&tree), "");
    }

    #[test]
    fn a_summary_marker_truncates_the_html_summary() {
        let doc = "`some_func(a, b)` -- does a thing.[/]The rest of the docs.";
        let summary = document_to_summary_html(doc, "some_func");
        assert!(summary.contains("ffdoc:some_func"), "{summary}");
        assert!(!summary.contains("The rest of the docs"), "{summary}");
    }

    #[test]
    fn a_multibyte_escaped_character_does_not_panic() {
        // `\é` -- the escaped character is multi-byte UTF-8, exercising
        // get_character's char-aware (not byte-aware) read.
        let tree = parse_document(r"caf\é", "test");
        let html = tree_to_html(&tree);
        assert_eq!(html, "caf\u{e9}");
    }
}
