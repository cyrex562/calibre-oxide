//! Port of `formatter_functions.py`'s `FORMATTING_VALUES` (8),
//! `DATE_FUNCTIONS` (3), and `URL_FUNCTIONS` (6) categories (issue
//! #518, part of the #460 formatter epic), ~17 total. Like
//! [`super::string_functions`]/[`super::list_functions`]/
//! [`super::numeric_functions`], none of these touch a book/`Cache`,
//! so they live here in `calibre_utils`.
//!
//! Of the 17, `f_string` was already #513's own inlined
//! `ExprKind::FString`; `format_date_field` (needs the real field
//! registry + a book's actual date value), `rating_to_stars` (needs
//! `calibre_ebooks::oeb::transforms::jacket::rating_to_stars`, and
//! `calibre_utils` can't depend on `calibre_ebooks` -- that crate
//! depends on this one), and `urls_from_identifiers` (needs
//! `calibre_ebooks::xml_util::prepare_string_for_xml` for the same
//! reason) all live in `calibre_db::formatter_functions` instead. The
//! remaining 13 are here.
//!
//! # A real Python `str.format()` mini-language engine, not a stub
//!
//! `format_number`/`finish_formatting` both need upstream's own
//! `_do_format`/`str.format()`-style format-spec engine (`[[fill]align]
//! [sign][#][0][width][,][.precision][type]`, e.g. `"05.2f"`,
//! `",d"`). Implements fill/align/sign/`#`/zero-pad/width/`,`-grouping/
//! precision for `s` (string), `b`/`o`/`d`/`x`/`X` (integer), and
//! `f`/`F`/`e`/`E`/`%`/`g`/`G` (float) types -- covers every example in
//! upstream's own docstrings and the overwhelming majority of real
//! templates. Disclosed narrowings: `n` (locale-aware number
//! formatting) is treated as unlocalized `d`/`g` (no locale-formatting
//! machinery exists in this crate); `g`/`G`'s significant-digit
//! selection is a close, tested approximation of CPython's algorithm,
//! not verified byte-identical for every edge case.
//!
//! # Real, deliberately preserved upstream quirks
//!
//! `make_url`/`make_url_extended`'s own error messages say "requires
//! an odd number of arguments" but the actual check requires an
//! *even* count (`len(args) % 2 != 0` is the error condition) -- a
//! real upstream wording/code mismatch, preserved verbatim rather than
//! silently corrected.

use super::interp::{float_deal_with_none, FunctionRegistry};
use super::numeric_functions::python_float_str;
use super::parser::FunctionCatalog;
use crate::date::{format_date, parse_date};
use chrono::{DateTime, Duration, Utc};

pub struct FormatFunctions;

impl FunctionRegistry for FormatFunctions {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String> {
        call(name, args)
    }
}

pub struct FormatCatalog;

impl FunctionCatalog for FormatCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>> {
        arg_count(name)
    }
}

pub fn arg_count(name: &str) -> Option<Option<usize>> {
    match name {
        "human_readable" => Some(Some(1)),
        "format_number" | "format_date" => Some(Some(2)),
        "finish_formatting" => Some(Some(4)),
        "format_duration" | "date_arithmetic" | "make_url" | "make_url_extended" | "query_string" => Some(None),
        "today" => Some(Some(0)),
        "days_between" | "encode_for_url" => Some(Some(2)),
        "to_hex" => Some(Some(1)),
        "urls_from_identifiers" => Some(Some(2)),
        _ => None,
    }
}

pub fn call(name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "human_readable" => Ok(human_readable(&args[0])),
        "format_number" => Ok(format_number(&args[0], &args[1])),
        "format_date" => Ok(format_date_fn(&args[0], &args[1])),
        "finish_formatting" => finish_formatting(&args[0], &args[1], &args[2], &args[3]),
        "format_duration" => format_duration(args),
        "today" => Ok(format_date(&Utc::now(), "iso")),
        "days_between" => Ok(days_between(&args[0], &args[1])),
        "date_arithmetic" => date_arithmetic(&args[0], &args[1], args.get(2).map(String::as_str).unwrap_or("")),
        "to_hex" => Ok(hex::encode(args[0].as_bytes())),
        "make_url" => make_url(args),
        "make_url_extended" => make_url_extended(args),
        "query_string" => query_string(args),
        "encode_for_url" => encode_for_url(&args[0], &args[1]),
        _ => Err(format!("No function named {name:?} exists")),
    }
}

/// Port of `qquote` (`calibre.ebooks.metadata.search_internet.qquote`):
/// percent-encode `val` as UTF-8, replacing spaces with `+` when
/// `use_plus` (matching Python's `quote_plus`) or `%20` otherwise
/// (matching `quote`). Disclosed narrowing: Python's `quote(val)`
/// (the `use_plus=False` path) defaults to `safe='/'` (`/` left
/// unescaped) -- the `urlencoding` crate always percent-encodes `/`
/// (`%2F`) in both paths. Real query *values* (this function's only
/// real use here) essentially never rely on an unescaped `/`.
pub(crate) fn qquote(val: &str, use_plus: bool) -> String {
    let encoded = urlencoding::encode(val).into_owned();
    if use_plus { encoded.replace("%20", "+") } else { encoded }
}

/// Port of `human_readable` (`calibre.__init__.human_readable`):
/// formats a byte count as `"<N>[.<d>] <unit>"` (B/KB/MB/.../EB),
/// truncating (not rounding) to one decimal digit and dropping a
/// trailing `.0` -- a real, literal string-slice operation in
/// upstream, not a rounding one.
fn human_readable(val: &str) -> String {
    let Some(raw) = float_deal_with_none(val) else { return String::new() };
    let size = raw.round() as i64;
    let units = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    let mut divisor: i64 = 1;
    let mut suffix = "B";
    for (i, u) in units.iter().enumerate() {
        let threshold = 1i64 << ((i as u32 + 1) * 10);
        if size < threshold {
            divisor = 1i64 << (i as u32 * 10);
            suffix = u;
            break;
        }
    }
    let quotient = size as f64 / divisor as f64;
    let full = python_float_str(quotient);
    let dot = full.find('.').expect("python_float_str always includes a decimal point");
    let truncated = &full[..(dot + 2).min(full.len())];
    let stripped = truncated.strip_suffix(".0").unwrap_or(truncated);
    format!("{stripped} {suffix}")
}

// ---- A real Python str.format()-style format-spec engine ----

#[derive(Debug, Clone, Copy, PartialEq)]
enum Align {
    Left,
    Right,
    Center,
    Zero,
}

struct Spec {
    fill: char,
    align: Option<Align>,
    sign: char,
    alternate: bool,
    width: Option<usize>,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

fn parse_spec(spec: &str) -> Result<Spec, String> {
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let mut fill = ' ';
    let mut align = None;
    let to_align = |c: char| match c {
        '<' => Align::Left,
        '>' => Align::Right,
        '^' => Align::Center,
        '=' => Align::Zero,
        _ => unreachable!(),
    };
    if chars.len() >= 2 && matches!(chars[1], '<' | '>' | '^' | '=') {
        fill = chars[0];
        align = Some(to_align(chars[1]));
        i = 2;
    } else if !chars.is_empty() && matches!(chars[0], '<' | '>' | '^' | '=') {
        align = Some(to_align(chars[0]));
        i = 1;
    }
    let mut sign = '-';
    if i < chars.len() && matches!(chars[i], '+' | '-' | ' ') {
        sign = chars[i];
        i += 1;
    }
    let mut alternate = false;
    if i < chars.len() && chars[i] == '#' {
        alternate = true;
        i += 1;
    }
    if i < chars.len() && chars[i] == '0' {
        i += 1;
        if align.is_none() {
            align = Some(Align::Zero);
            fill = '0';
        }
    }
    let width_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    let width = (i > width_start).then(|| chars[width_start..i].iter().collect::<String>().parse().unwrap());
    let mut grouping = None;
    if i < chars.len() && matches!(chars[i], ',' | '_') {
        grouping = Some(chars[i]);
        i += 1;
    }
    let mut precision = None;
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        let p_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == p_start {
            return Err(format!("Format specifier missing precision: '{spec}'"));
        }
        precision = Some(chars[p_start..i].iter().collect::<String>().parse().unwrap());
    }
    let ty = (i < chars.len()).then(|| {
        let c = chars[i];
        i += 1;
        c
    });
    if i != chars.len() {
        return Err(format!("Invalid format specifier '{spec}'"));
    }
    Ok(Spec { fill, align, sign, alternate, width, grouping, precision, ty })
}

fn pad(body: &str, spec: &Spec, numeric: bool) -> String {
    let Some(width) = spec.width else { return body.to_string() };
    let len = body.chars().count();
    if len >= width {
        return body.to_string();
    }
    let fill_count = width - len;
    let align = spec.align.unwrap_or(if numeric { Align::Right } else { Align::Left });
    let fill: String = spec.fill.to_string();
    match align {
        Align::Left => format!("{body}{}", fill.repeat(fill_count)),
        Align::Right => format!("{}{body}", fill.repeat(fill_count)),
        Align::Center => {
            let left = fill_count / 2;
            let right = fill_count - left;
            format!("{}{body}{}", fill.repeat(left), fill.repeat(right))
        }
        Align::Zero => match body.chars().next() {
            Some(c @ ('+' | '-' | ' ')) => format!("{c}{}{}", "0".repeat(fill_count), &body[c.len_utf8()..]),
            _ => format!("{}{body}", "0".repeat(fill_count)),
        },
    }
}

fn sign_str(negative: bool, mode: char) -> &'static str {
    if negative {
        "-"
    } else {
        match mode {
            '+' => "+",
            ' ' => " ",
            _ => "",
        }
    }
}

fn group_thousands(digits: &str) -> String {
    let chars: Vec<char> = digits.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    for (idx, c) in chars.into_iter().enumerate() {
        if idx > 0 && (n - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn group_float_thousands(s: &str) -> String {
    match s.find('.') {
        Some(dot) => format!("{}{}", group_thousands(&s[..dot]), &s[dot..]),
        None => group_thousands(s),
    }
}

fn format_int_body(v: i64, spec: &Spec) -> Result<String, String> {
    if spec.precision.is_some() {
        return Err("Precision not allowed in integer format specifier".to_string());
    }
    let ty = spec.ty.unwrap_or('d');
    let neg = v < 0;
    let abs = v.unsigned_abs();
    let mut digits = match ty {
        'd' | 'n' => abs.to_string(),
        'b' => format!("{abs:b}"),
        'o' => format!("{abs:o}"),
        'x' => format!("{abs:x}"),
        'X' => format!("{abs:X}"),
        'c' => return Ok(pad(&char::from_u32(abs as u32).map(|c| c.to_string()).unwrap_or_default(), spec, false)),
        _ => return Err(format!("Unknown format code '{ty}' for integer")),
    };
    if spec.grouping == Some(',') && (ty == 'd' || ty == 'n') {
        digits = group_thousands(&digits);
    }
    if spec.alternate {
        let prefix = match ty {
            'b' => "0b",
            'o' => "0o",
            'x' => "0x",
            'X' => "0X",
            _ => "",
        };
        digits = format!("{prefix}{digits}");
    }
    Ok(pad(&format!("{}{digits}", sign_str(neg, spec.sign)), spec, true))
}

fn format_scientific(abs: f64, precision: usize, upper: bool) -> String {
    if abs == 0.0 {
        return format!("{:.*}{}+00", precision, 0.0, if upper { "E" } else { "e" });
    }
    let exp0 = abs.log10().floor() as i32;
    let mantissa0 = abs / 10f64.powi(exp0);
    let (mantissa, exp) = if format!("{mantissa0:.*}", precision).starts_with("10") { (mantissa0 / 10.0, exp0 + 1) } else { (mantissa0, exp0) };
    format!("{:.*}{}{}{:02}", precision, mantissa, if upper { "E" } else { "e" }, if exp < 0 { '-' } else { '+' }, exp.abs())
}

fn strip_trailing_zeros_fixed(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn strip_trailing_zeros_scientific(s: &str) -> String {
    match s.find(['e', 'E']) {
        Some(epos) => format!("{}{}", strip_trailing_zeros_fixed(&s[..epos]), &s[epos..]),
        None => s.to_string(),
    }
}

/// A close approximation of CPython's `%g`/`%G` significant-digit
/// algorithm -- see this module's own doc for the disclosed narrowing.
fn format_general(abs: f64, precision: usize, upper: bool) -> String {
    let precision = precision.max(1);
    if abs == 0.0 {
        return "0".to_string();
    }
    let exp = abs.log10().floor() as i32;
    if exp < -4 || exp >= precision as i32 {
        strip_trailing_zeros_scientific(&format_scientific(abs, precision.saturating_sub(1), upper))
    } else {
        let decimals = (precision as i32 - 1 - exp).max(0) as usize;
        strip_trailing_zeros_fixed(&format!("{abs:.decimals$}"))
    }
}

fn format_float_body(v: f64, spec: &Spec) -> Result<String, String> {
    let ty = spec.ty.unwrap_or('g');
    let neg = v.is_sign_negative() && v != 0.0;
    let abs = v.abs();
    let precision = spec.precision;
    let mut digits = match ty {
        'f' => format!("{:.*}", precision.unwrap_or(6), abs),
        'F' => format!("{:.*}", precision.unwrap_or(6), abs).to_uppercase(),
        '%' => format!("{:.*}%", precision.unwrap_or(6), abs * 100.0),
        'e' => format_scientific(abs, precision.unwrap_or(6), false),
        'E' => format_scientific(abs, precision.unwrap_or(6), true),
        'g' => format_general(abs, precision.unwrap_or(6), false),
        'G' => format_general(abs, precision.unwrap_or(6), true),
        _ => return Err(format!("Unknown format code '{ty}' for float")),
    };
    if spec.grouping == Some(',') && matches!(ty, 'f' | 'F') {
        digits = group_float_thousands(&digits);
    }
    Ok(pad(&format!("{}{digits}", sign_str(neg, spec.sign)), spec, true))
}

fn format_string_body(s: &str, spec: &Spec) -> Result<String, String> {
    if spec.sign != '-' || spec.alternate {
        return Err("Invalid format specifier for string".to_string());
    }
    let truncated: String = match spec.precision {
        Some(p) => s.chars().take(p).collect(),
        None => s.to_string(),
    };
    Ok(pad(&truncated, spec, false))
}

enum FormatValue<'a> {
    Str(&'a str),
    Int(i64),
    Float(f64),
}

fn format_value(value: FormatValue, spec_str: &str) -> Result<String, String> {
    let spec = parse_spec(spec_str)?;
    match value {
        FormatValue::Int(v) => format_int_body(v, &spec),
        FormatValue::Float(v) => format_float_body(v, &spec),
        FormatValue::Str(s) => format_string_body(s, &spec),
    }
}

/// Port of `formatter.py`'s `_Interpreter._do_format`: dispatches on
/// the format spec's own trailing type character to decide whether
/// `raw` should be parsed as an int, a float, or left as a string.
fn do_format(raw: &str, spec_str: &str) -> Result<String, String> {
    if spec_str.is_empty() || raw.is_empty() {
        return Ok(raw.to_string());
    }
    let ty_char = spec_str.chars().last().unwrap();
    if "bcdoxXn".contains(ty_char) {
        let v: i64 = raw.parse().map_err(|_| format!("format: type {ty_char} requires an integer value, got {raw}"))?;
        format_value(FormatValue::Int(v), spec_str)
    } else if "eEfFgGn%".contains(ty_char) {
        let v: f64 = raw.parse().map_err(|_| format!("format: type {ty_char} requires a decimal (float) value, got {raw}"))?;
        format_value(FormatValue::Float(v), spec_str)
    } else {
        format_value(FormatValue::Str(raw), spec_str)
    }
}

fn extract_format_spec(template: &str) -> Result<String, String> {
    let inner = template.strip_prefix('{').and_then(|s| s.strip_suffix('}')).ok_or_else(|| format!("invalid template {template:?}"))?;
    Ok(inner.strip_prefix("0:").unwrap_or(inner).to_string())
}

/// Port of `format_number`: tries the value as a float first (real
/// upstream semantics -- an int-type spec like `"d"` genuinely fails
/// against a float value in Python's own `str.format`), falling back
/// to a truncated int only when the value has no fractional part.
fn format_number(val: &str, template: &str) -> String {
    if val.is_empty() || val == "None" {
        return String::new();
    }
    let template = if template.contains('{') { template.to_string() } else { format!("{{0:{template}}}") };
    let Ok(spec_str) = extract_format_spec(&template) else { return String::new() };
    let Ok(v1) = val.parse::<f64>() else { return String::new() };
    if let Ok(s) = format_value(FormatValue::Float(v1), &spec_str) {
        return s;
    }
    let v2 = v1.trunc();
    if v2 == v1 {
        if let Ok(s) = format_value(FormatValue::Int(v2 as i64), &spec_str) {
            return s;
        }
    }
    String::new()
}

/// Port of `finish_formatting`.
fn finish_formatting(val: &str, fmt: &str, prefix: &str, suffix: &str) -> Result<String, String> {
    if val.is_empty() {
        return Ok(val.to_string());
    }
    Ok(format!("{prefix}{}{suffix}", do_format(val, fmt)?))
}

/// Shared `to_number`/`from_number`/plain-format dispatch used by both
/// [`format_date_fn`] here and `calibre_db::formatter_functions`'s
/// `format_date_field` (which already has a real, parsed date and
/// just needs the same three-way format-string handling).
///
/// Disclosed narrowing: `from_number`'s epoch-seconds interpretation
/// is UTC here, matching upstream's `datetime.fromtimestamp` only when
/// the process's local timezone happens to be UTC -- no ambient
/// local-timezone source exists in this crate to do otherwise (the
/// same narrowing this crate's `convert_to_local_tz`/`get_data_as_dict`
/// call sites already disclose elsewhere).
pub fn format_parsed_date(d: DateTime<Utc>, format_string: &str) -> Option<String> {
    if format_string == "to_number" {
        Some(python_float_str(d.timestamp() as f64))
    } else if format_string.starts_with("from_number") {
        let f = format_string.get(12..).unwrap_or("");
        Some(format_date(&d, if f.is_empty() { "iso" } else { f }))
    } else {
        Some(format_date(&d, format_string))
    }
}

/// Port of `format_date`.
fn format_date_fn(val: &str, format_string: &str) -> String {
    if val.is_empty() || val == "None" {
        return String::new();
    }
    let result = if let Some(f) = format_string.strip_prefix("from_number") {
        val.parse::<f64>().ok().and_then(|ts| {
            let secs = ts.floor() as i64;
            let nanos = ((ts - ts.floor()) * 1e9).round() as u32;
            DateTime::from_timestamp(secs, nanos).map(|d| format_parsed_date(d, &format!("from_number{f}")).unwrap())
        })
    } else {
        parse_date(val, true).and_then(|d| format_parsed_date(d, format_string))
    };
    result.unwrap_or_else(|| "BAD DATE".to_string())
}

/// Port of `today`'s helper is inlined in [`call`] (`format_date(now(), "iso")`).
///
/// Port of `days_between`. Real upstream computes `i.days + i.seconds
/// / 86400.0` from a Python `timedelta` -- mathematically identical
/// to `total_seconds / 86400.0` (a `timedelta`'s `days`/`seconds`
/// fields are just a floor-division decomposition of the same total),
/// so this is a real structural simplification, not a narrowing.
fn days_between(date1: &str, date2: &str) -> String {
    let Some(d1) = parse_date(date1, true) else { return String::new() };
    let Some(d2) = parse_date(date2, true) else { return String::new() };
    let seconds = (d1 - d2).num_milliseconds() as f64 / 1000.0;
    format!("{:.1}", seconds / 86400.0)
}

/// Port of `date_arithmetic`.
fn date_arithmetic(value: &str, calc_spec: &str, fmt: &str) -> Result<String, String> {
    let Some(mut d) = parse_date(value, true) else { return Ok(String::new()) };
    let re = regex::Regex::new(r"^([-+]?\d+)([smhdwy])").unwrap();
    let mut remaining = calc_spec;
    while !remaining.is_empty() {
        let Some(caps) = re.captures(remaining) else {
            return Err(format!("date_arithmetic: invalid calculation specifier '{remaining}'"));
        };
        let n: i64 = caps[1].parse().map_err(|_| format!("date_arithmetic: invalid number in '{remaining}'"))?;
        let delta = match &caps[2] {
            "s" => Duration::seconds(n),
            "m" => Duration::minutes(n),
            "h" => Duration::hours(n),
            "d" => Duration::days(n),
            "w" => Duration::weeks(n),
            "y" => Duration::days(n * 365),
            _ => unreachable!(),
        };
        d += delta;
        remaining = &remaining[caps[0].len()..];
    }
    Ok(format_date(&d, if fmt.is_empty() { "iso" } else { fmt }))
}

/// Port of `format_duration`.
fn format_duration(args: &[String]) -> Result<String, String> {
    if args.len() < 2 {
        return Err("format_duration requires at least 2 arguments".to_string());
    }
    let value = &args[0];
    let template = &args[1];
    let largest_unit_arg = args.get(2).map(String::as_str).unwrap_or("");
    if !largest_unit_arg.is_empty() && !"wdhms".contains(largest_unit_arg) {
        return Err("format_duration: the largest_unit parameter must be one of wdhms".to_string());
    }

    let pat = regex::Regex::new(r"\[(.)(?::(.*?))?\]").unwrap();
    let largest_unit = if largest_unit_arg.is_empty() {
        let mut highest_index = 0usize;
        for caps in pat.captures_iter(template) {
            let c = caps[1].to_lowercase();
            let dex = "smhdw".find(c.as_str()).ok_or_else(|| format!("The {} format specifier is not valid", &caps[0]))?;
            highest_index = highest_index.max(dex);
        }
        "smhdw".chars().nth(highest_index).unwrap()
    } else {
        largest_unit_arg.chars().next().unwrap()
    };

    let int_val = if !value.is_empty() { float_deal_with_none(value).unwrap_or(0.0).round() as i64 } else { 0 };
    let mut remainder = int_val;
    let (weeks, r) = if largest_unit == 'w' { (remainder.div_euclid(604800), remainder.rem_euclid(604800)) } else { (-1, remainder) };
    remainder = r;
    let (days, r) = if "wd".contains(largest_unit) { (remainder.div_euclid(86400), remainder.rem_euclid(86400)) } else { (-1, remainder) };
    remainder = r;
    let (hours, r) = if "wdh".contains(largest_unit) { (remainder.div_euclid(3600), remainder.rem_euclid(3600)) } else { (-1, remainder) };
    remainder = r;
    let (minutes, r) = if "wdhm".contains(largest_unit) { (remainder.div_euclid(60), remainder.rem_euclid(60)) } else { (-1, remainder) };
    let seconds = r;

    let val_with_suffix = |val: i64, test_val: i64, fmt_char: char, zero_suffix: &str, one_suffix: &str, more_suffix: &str| -> String {
        match val {
            -1 => String::new(),
            0 if fmt_char.is_lowercase() && int_val < test_val => String::new(),
            0 => format!("0{zero_suffix}"),
            1 => format!("1{one_suffix}"),
            v => format!("{v}{more_suffix}"),
        }
    };

    let mut err: Option<String> = None;
    let mut last_end = 0;
    let mut out = String::new();
    for caps in pat.captures_iter(template) {
        let m = caps.get(0).unwrap();
        out.push_str(&template[last_end..m.start()]);
        last_end = m.end();
        let fmt_char = caps[1].chars().next().unwrap();
        let (zero_suffix, one_suffix, more_suffix) = match caps.get(2) {
            None => {
                let s = format!("{} ", fmt_char.to_lowercase());
                (s.clone(), s.clone(), s)
            }
            Some(m) => {
                let parts: Vec<&str> = m.as_str().split('|').collect();
                match parts.len() {
                    1 => (parts[0].to_string(), parts[0].to_string(), parts[0].to_string()),
                    2 => (parts[0].to_string(), parts[1].to_string(), parts[0].to_string()),
                    3 => (parts[0].to_string(), parts[1].to_string(), parts[2].to_string()),
                    _ => {
                        err = Some(format!("The group {fmt_char} has too many suffixes"));
                        (String::new(), String::new(), String::new())
                    }
                }
            }
        };
        let rendered = match fmt_char.to_ascii_lowercase() {
            'w' => val_with_suffix(weeks, 604800, fmt_char, &zero_suffix, &one_suffix, &more_suffix),
            'd' => val_with_suffix(days, 86400, fmt_char, &zero_suffix, &one_suffix, &more_suffix),
            'h' => val_with_suffix(hours, 3600, fmt_char, &zero_suffix, &one_suffix, &more_suffix),
            'm' => val_with_suffix(minutes, 60, fmt_char, &zero_suffix, &one_suffix, &more_suffix),
            's' => val_with_suffix(seconds, -1, fmt_char, &zero_suffix, &one_suffix, &more_suffix),
            _ => {
                err = Some(format!("The {fmt_char} format specifier is not valid"));
                String::new()
            }
        };
        out.push_str(&rendered);
    }
    out.push_str(&template[last_end..]);
    if let Some(e) = err {
        return Err(e);
    }
    Ok(out)
}

/// Port of `make_url` -- see this module's own doc for the real
/// "odd"-worded/even-checking error message quirk, preserved verbatim.
fn make_url(args: &[String]) -> Result<String, String> {
    if args.is_empty() {
        return Err("make_url requires at least 3 arguments".to_string());
    }
    let path = &args[0];
    let rest = &args[1..];
    if rest.len() % 2 != 0 {
        return Err("make_url requires an odd number of arguments".to_string());
    }
    if rest.len() < 2 {
        return Err("make_url requires at least 3 arguments".to_string());
    }
    let mut parts = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        parts.push(format!("{}={}", rest[i], qquote(rest[i + 1].trim(), true)));
        i += 2;
    }
    Ok(format!("{path}?{}", parts.join("&")))
}

/// Port of `make_url_extended`.
fn make_url_extended(args: &[String]) -> Result<String, String> {
    if args.len() < 3 {
        return Err("make_url_extended requires at least 5 arguments".to_string());
    }
    let scheme = &args[0];
    let authority = &args[1];
    let path = &args[2];
    let rest = &args[3..];
    let qs = if rest.len() == 1 {
        rest[0].clone()
    } else {
        if rest.len() % 2 != 0 {
            return Err("make_url_extended requires an odd number of arguments".to_string());
        }
        if rest.len() < 2 {
            return Err("make_url_extended requires at least 5 arguments".to_string());
        }
        let mut parts = Vec::new();
        let mut i = 0;
        while i < rest.len() {
            parts.push(format!("{}={}", rest[i], qquote(rest[i + 1].trim(), true)));
            i += 2;
        }
        parts.join("&")
    };
    let qs = if qs.is_empty() { qs } else { format!("?{qs}") };
    let slash = if authority.is_empty() { "" } else { "/" };
    Ok(format!("{scheme}://{authority}{slash}{}{qs}", path.strip_prefix('/').unwrap_or(path)))
}

/// Port of `query_string`.
fn query_string(args: &[String]) -> Result<String, String> {
    if args.len() % 3 != 0 || args.len() < 3 {
        return Err("query_string requires at least one group of 3 arguments".to_string());
    }
    let mut parts = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let name = &args[i];
        let value = args[i + 1].trim();
        let how = &args[i + 2];
        let encoded = match how.as_str() {
            "0" => qquote(value, true),
            "1" => qquote(value, false),
            "2" => value.to_string(),
            _ => return Err(format!("In query_string the third argument of a group must be 0, 1, or 2, not {how}")),
        };
        parts.push(format!("{name}={encoded}"));
        i += 3;
    }
    Ok(parts.join("&"))
}

/// Port of `encode_for_url` -- note the parameter's real, slightly
/// counter-intuitive semantics preserved verbatim: `use_plus == "0"`
/// actually means "yes, use plus signs".
fn encode_for_url(value: &str, use_plus: &str) -> Result<String, String> {
    if use_plus != "0" && use_plus != "1" {
        return Err(format!("In encode_for_url the second argument must be 0, or 1, not {use_plus}"));
    }
    Ok(qquote(value, use_plus == "0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_readable_matches_documented_units() {
        assert_eq!(call("human_readable", &["500".to_string()]).unwrap(), "500 B");
        assert_eq!(call("human_readable", &["1536".to_string()]).unwrap(), "1.5 KB");
        assert_eq!(call("human_readable", &["1073741824".to_string()]).unwrap(), "1 GB");
    }

    #[test]
    fn format_number_matches_documented_templates() {
        assert_eq!(call("format_number", &["3".to_string(), "5.2f".to_string()]).unwrap(), " 3.00");
        assert_eq!(call("format_number", &["1234".to_string(), ",d".to_string()]).unwrap(), "1,234");
        assert_eq!(call("format_number", &["".to_string(), "d".to_string()]).unwrap(), "");
        assert_eq!(call("format_number", &["notanumber".to_string(), "d".to_string()]).unwrap(), "");
    }

    #[test]
    fn format_number_falls_back_to_int_only_when_whole() {
        assert_eq!(call("format_number", &["4".to_string(), "d".to_string()]).unwrap(), "4", "4.0 has no fraction, so the int fallback kicks in");
        assert_eq!(call("format_number", &["4.5".to_string(), "d".to_string()]).unwrap(), "", "a real fraction can't format as an int either");
    }

    #[test]
    fn finish_formatting_applies_format_prefix_and_suffix() {
        assert_eq!(call("finish_formatting", &["3".to_string(), "05.2f".to_string(), "[".to_string(), "]".to_string()]).unwrap(), "[03.00]");
        assert_eq!(call("finish_formatting", &["".to_string(), "05.2f".to_string(), "[".to_string(), "]".to_string()]).unwrap(), "", "an empty value is returned unchanged");
    }

    #[test]
    fn today_and_format_date_round_trip() {
        let now = call("today", &[]).unwrap();
        assert!(!now.is_empty());
        assert_eq!(call("format_date", &["2020-06-15".to_string(), "yyyy-MM-dd".to_string()]).unwrap(), "2020-06-15");
        assert_eq!(call("format_date", &["not a date".to_string(), "yyyy".to_string()]).unwrap(), "BAD DATE");
    }

    #[test]
    fn days_between_is_positive_when_date1_is_later() {
        assert_eq!(call("days_between", &["2020-01-03".to_string(), "2020-01-01".to_string()]).unwrap(), "2.0");
        assert_eq!(call("days_between", &["2020-01-01".to_string(), "2020-01-03".to_string()]).unwrap(), "-2.0");
        assert_eq!(call("days_between", &["not a date".to_string(), "2020-01-01".to_string()]).unwrap(), "");
    }

    #[test]
    fn date_arithmetic_applies_each_calc_spec_segment() {
        assert_eq!(call("date_arithmetic", &["2020-01-01".to_string(), "1d".to_string(), "yyyy-MM-dd".to_string()]).unwrap(), "2020-01-02");
        assert_eq!(call("date_arithmetic", &["2020-01-10".to_string(), "-5d".to_string(), "yyyy-MM-dd".to_string()]).unwrap(), "2020-01-05");
        assert!(call("date_arithmetic", &["2020-01-01".to_string(), "bogus".to_string(), "".to_string()]).is_err());
    }

    #[test]
    fn format_duration_matches_documented_examples() {
        // Each default (no custom suffix) selector expands to
        // `"<n><letter> "` (WITH a trailing space) -- upstream's own
        // docstring examples drop that trailing space when displayed,
        // but the real `evaluate()` body never strips it.
        assert_eq!(call("format_duration", &["176420".to_string(), "[d][h][m][s]".to_string()]).unwrap(), "2d 1h 0m 20s ");
        assert_eq!(call("format_duration", &["176420".to_string(), "[h][m][s]".to_string()]).unwrap(), "49h 0m 20s ");
        assert_eq!(call("format_duration", &["176420".to_string(), "[W][d][h][m][s]".to_string()]).unwrap(), "0w 2d 1h 0m 20s ");
        assert_eq!(call("format_duration", &["176420".to_string(), "[h][m][s]".to_string(), "d".to_string()]).unwrap(), "1h 0m 20s ");
    }

    #[test]
    fn to_hex_encodes_utf8_bytes() {
        assert_eq!(call("to_hex", &["AB".to_string()]).unwrap(), "4142");
    }

    #[test]
    fn make_url_builds_a_query_from_pairs() {
        assert_eq!(call("make_url", &["https://example.com".to_string(), "q".to_string(), "hello world".to_string()]).unwrap(), "https://example.com?q=hello+world");
        assert!(call("make_url", &["https://example.com".to_string()]).is_err());
    }

    #[test]
    fn make_url_extended_supports_pairs_and_a_raw_query_string() {
        assert_eq!(call("make_url_extended", &["https".to_string(), "example.com".to_string(), "/p".to_string(), "q".to_string(), "hi".to_string()]).unwrap(), "https://example.com/p?q=hi");
        assert_eq!(call("make_url_extended", &["https".to_string(), "example.com".to_string(), "/p".to_string(), "q=hi".to_string()]).unwrap(), "https://example.com/p?q=hi");
        assert_eq!(call("make_url_extended", &["calibre".to_string(), "".to_string(), "/p".to_string(), "".to_string()]).unwrap(), "calibre://p");
    }

    #[test]
    fn query_string_supports_all_three_encoding_modes() {
        assert_eq!(call("query_string", &["a".to_string(), "hi there".to_string(), "0".to_string()]).unwrap(), "a=hi+there");
        assert_eq!(call("query_string", &["a".to_string(), "hi there".to_string(), "1".to_string()]).unwrap(), "a=hi%20there");
        assert_eq!(call("query_string", &["a".to_string(), "hi there".to_string(), "2".to_string()]).unwrap(), "a=hi there");
        assert!(call("query_string", &["a".to_string(), "b".to_string(), "9".to_string()]).is_err());
    }

    #[test]
    fn encode_for_url_uses_the_real_inverted_use_plus_semantics() {
        assert_eq!(call("encode_for_url", &["hi there".to_string(), "0".to_string()]).unwrap(), "hi+there");
        assert_eq!(call("encode_for_url", &["hi there".to_string(), "1".to_string()]).unwrap(), "hi%20there");
        assert!(call("encode_for_url", &["x".to_string(), "9".to_string()]).is_err());
    }

    #[test]
    fn unknown_function_is_a_real_error() {
        assert!(call("no_such_function", &[]).is_err());
    }

    #[test]
    fn catalog_reports_correct_arity() {
        assert_eq!(arg_count("human_readable"), Some(Some(1)));
        assert_eq!(arg_count("make_url"), Some(None));
        assert_eq!(arg_count("no_such_function"), None);
    }
}
