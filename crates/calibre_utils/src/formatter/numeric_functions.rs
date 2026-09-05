//! Port of `formatter_functions.py`'s `ARITHMETIC` (9), `RELATIONAL`
//! (4), and `BOOLEAN` (3) categories (issue #517, part of the #460
//! formatter epic), ~16 total. Like [`super::string_functions`]/
//! [`super::list_functions`], none of these touch a book/`Cache`, so
//! they live here in `calibre_utils`.
//!
//! # A real bug in this crate's own `icu::strcmp` found while porting `strcmp`/`strcmpcase`
//!
//! Porting `strcmp`/`strcmpcase` side by side made a pre-existing bug
//! in `calibre_utils::icu::strcmp` (from issue #459) impossible to
//! miss: it used ICU4X's own default collator strength (`Tertiary`,
//! which distinguishes case), but real upstream's `strcmp` is built
//! from `sort_collator()`, explicitly `Strength::Secondary`
//! ("Ignores case differences...", straight from that function's own
//! docstring) -- confirmed by direct probing
//! (`Collator::compare("apple", "Apple")` is `Equal` at `Secondary`,
//! not at the default/`Tertiary` strength this crate used before).
//! Fixed `icu::strcmp` itself (now `Secondary`) and added a real
//! `icu::case_sensitive_strcmp` (`CaseFirst::UpperFirst`, matching
//! upstream's separate `case_sensitive_collator()`) -- see `icu.rs`'s
//! own doc on `strcmp` for the full story. This also *corrects* a
//! wrong conclusion drawn while porting #516's `str_in_list` (which
//! had (wrongly) concluded upstream's own docstring was inaccurate --
//! it wasn't; this crate's `strcmp` was).
//!
//! # Real Python numeric-formatting semantics preserved, not "fixed"
//!
//! `add`/`subtract`/`multiply`/`divide` return Python's raw
//! `str(float)` (always shows a trailing `.0` for a whole-number
//! result, e.g. `add(2, 3)` -> `"5.0"`) -- a *different* convention
//! from this crate's own `format_number` (used by the `+`/`-`/`*`/`/`
//! *operators*, which suppress the trailing `.0`). See [`python_float_str`].
//! `ceiling`/`floor`/`round` return Python's `math.ceil`/`math.floor`/
//! `round()` semantics, which return a real Python `int` (bare integer
//! string, never `.0`) -- `round()` specifically uses round-half-to-
//! even ("banker's rounding"), matched here with `f64::round_ties_even`
//! (stable since Rust 1.77), not `f64::round` (round-half-away-from-zero).
//! `mod`'s `value % y` follows Python's floored-modulo sign convention
//! (result takes the sign of `y`), unlike Rust's `%` operator (which
//! follows the sign of the dividend, like C).

use super::interp::{float_deal_with_none, FunctionRegistry};
use super::parser::FunctionCatalog;
use crate::icu;

pub struct NumericFunctions;

impl FunctionRegistry for NumericFunctions {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String> {
        call(name, args)
    }
}

pub struct NumericCatalog;

impl FunctionCatalog for NumericCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>> {
        arg_count(name)
    }
}

pub fn arg_count(name: &str) -> Option<Option<usize>> {
    match name {
        "strcmp" | "strcmpcase" | "cmp" => Some(Some(5)),
        "first_matching_cmp" => Some(None),
        "add" | "multiply" => Some(None),
        "subtract" | "divide" | "mod" => Some(Some(2)),
        "ceiling" | "floor" | "round" | "fractional_part" | "not" => Some(Some(1)),
        "and" | "or" => Some(None),
        _ => None,
    }
}

pub fn call(name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "strcmp" => Ok(three_way(icu::strcmp(&args[0], &args[1]), &args[2], &args[3], &args[4])),
        "strcmpcase" => Ok(three_way(icu::case_sensitive_strcmp(&args[0], &args[1]), &args[2], &args[3], &args[4])),
        "cmp" => cmp(&args[0], &args[1], &args[2], &args[3], &args[4]),
        "first_matching_cmp" => first_matching_cmp(args),
        "add" => add(args),
        "subtract" => subtract(&args[0], &args[1]),
        "multiply" => multiply(args),
        "divide" => divide(&args[0], &args[1]),
        "ceiling" => ceiling(&args[0]),
        "floor" => floor(&args[0]),
        "round" => round(&args[0]),
        "mod" => mod_fn(&args[0], &args[1]),
        "fractional_part" => fractional_part(&args[0]),
        "and" => Ok(if args.iter().all(|a| !a.is_empty()) { "1".to_string() } else { String::new() }),
        "or" => Ok(if args.iter().any(|a| !a.is_empty()) { "1".to_string() } else { String::new() }),
        "not" => Ok(if args[0].is_empty() { "1".to_string() } else { String::new() }),
        _ => Err(format!("No function named {name:?} exists")),
    }
}

fn to_f64(fn_name: &str, s: &str) -> Result<f64, String> {
    float_deal_with_none(s).ok_or_else(|| format!("{fn_name}: '{s}' is not a number"))
}

fn three_way(ord: std::cmp::Ordering, lt: &str, eq: &str, gt: &str) -> String {
    match ord {
        std::cmp::Ordering::Less => lt.to_string(),
        std::cmp::Ordering::Equal => eq.to_string(),
        std::cmp::Ordering::Greater => gt.to_string(),
    }
}

/// Port of `cmp`.
fn cmp(value: &str, y: &str, lt: &str, eq: &str, gt: &str) -> Result<String, String> {
    let value = to_f64("cmp", value)?;
    let y = to_f64("cmp", y)?;
    Ok(if value < y {
        lt.to_string()
    } else if value == y {
        eq.to_string()
    } else {
        gt.to_string()
    })
}

/// Port of `first_matching_cmp`.
fn first_matching_cmp(args: &[String]) -> Result<String, String> {
    if args.len() % 2 != 0 {
        return Err("first_matching_cmp requires an even number of arguments".to_string());
    }
    if args.len() < 2 {
        return Err("first_matching_cmp requires at least a value and an else_result".to_string());
    }
    let val = to_f64("first_matching_cmp", &args[0])?;
    let mut i = 1;
    while i < args.len() - 1 {
        let c = to_f64("first_matching_cmp", &args[i])?;
        if val < c {
            return Ok(args[i + 1].clone());
        }
        i += 2;
    }
    Ok(args[args.len() - 1].clone())
}

/// Port of Python's `str(float)`: unlike this crate's own
/// `format_number` (used by the numeric operators), a whole-number
/// result always keeps a trailing `.0` -- see this module's own doc.
fn python_float_str(v: f64) -> String {
    if v.is_nan() {
        "nan".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "inf".to_string() } else { "-inf".to_string() }
    } else if v.fract() == 0.0 && v.abs() < 1e16 {
        format!("{v:.1}")
    } else {
        v.to_string()
    }
}

/// Port of `add`: real Python semantics where zero arguments leaves
/// the running total as the Python `int` `0` (`"0"`, no `.0`), but
/// summing at least one argument promotes it to a `float`.
fn add(args: &[String]) -> Result<String, String> {
    if args.is_empty() {
        return Ok("0".to_string());
    }
    let mut res = 0.0;
    for a in args {
        res += to_f64("add", a)?;
    }
    Ok(python_float_str(res))
}

/// Port of `subtract`.
fn subtract(x: &str, y: &str) -> Result<String, String> {
    Ok(python_float_str(to_f64("subtract", x)? - to_f64("subtract", y)?))
}

/// Port of `multiply` -- see [`add`]'s own doc for the zero-argument
/// case (here, the Python `int` `1`).
fn multiply(args: &[String]) -> Result<String, String> {
    if args.is_empty() {
        return Ok("1".to_string());
    }
    let mut res = 1.0;
    for a in args {
        res *= to_f64("multiply", a)?;
    }
    Ok(python_float_str(res))
}

/// Port of `divide`.
fn divide(x: &str, y: &str) -> Result<String, String> {
    let x = to_f64("divide", x)?;
    let y = to_f64("divide", y)?;
    if y == 0.0 {
        return Err("divide: division by zero".to_string());
    }
    Ok(python_float_str(x / y))
}

/// Port of `ceiling`: Python's `math.ceil` on a float returns an
/// `int`.
fn ceiling(value: &str) -> Result<String, String> {
    Ok((to_f64("ceiling", value)?.ceil() as i64).to_string())
}

/// Port of `floor`.
fn floor(value: &str) -> Result<String, String> {
    Ok((to_f64("floor", value)?.floor() as i64).to_string())
}

/// Port of `round`: Python's `round()` without an `ndigits` argument
/// uses round-half-to-even, not round-half-away-from-zero.
fn round(value: &str) -> Result<String, String> {
    Ok((to_f64("round", value)?.round_ties_even() as i64).to_string())
}

/// Port of `mod`: Python's `%` on floats follows the sign of the
/// divisor (floored division), unlike Rust's `%` (sign of the
/// dividend).
fn mod_fn(value: &str, y: &str) -> Result<String, String> {
    let value = to_f64("mod", value)?;
    let y = to_f64("mod", y)?;
    if y == 0.0 {
        return Err("mod: division by zero".to_string());
    }
    let mut r = value % y;
    if r != 0.0 && (r < 0.0) != (y < 0.0) {
        r += y;
    }
    Ok((r as i64).to_string())
}

/// Port of `fractional_part`: `math.modf(value)[0]`, which keeps the
/// sign of `value` -- matches Rust's `f64::fract` directly.
fn fractional_part(value: &str) -> Result<String, String> {
    Ok(python_float_str(to_f64("fractional_part", value)?.fract()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strcmp_is_case_insensitive_and_strcmpcase_is_not() {
        assert_eq!(call("strcmp", &["apple".to_string(), "Apple".to_string(), "lt".to_string(), "eq".to_string(), "gt".to_string()]).unwrap(), "eq");
        assert_eq!(call("strcmpcase", &["apple".to_string(), "Apple".to_string(), "lt".to_string(), "eq".to_string(), "gt".to_string()]).unwrap(), "gt", "lowercase sorts after uppercase under case_sensitive_strcmp's upper-first ordering");
        assert_eq!(call("strcmp", &["a".to_string(), "b".to_string(), "lt".to_string(), "eq".to_string(), "gt".to_string()]).unwrap(), "lt");
    }

    #[test]
    fn cmp_compares_numerically_not_lexically() {
        assert_eq!(call("cmp", &["9".to_string(), "10".to_string(), "lt".to_string(), "eq".to_string(), "gt".to_string()]).unwrap(), "lt", "numeric: 9 < 10, unlike lexical '9' > '10'");
        assert_eq!(call("cmp", &["5".to_string(), "5".to_string(), "lt".to_string(), "eq".to_string(), "gt".to_string()]).unwrap(), "eq");
        assert!(call("cmp", &["x".to_string(), "5".to_string(), "lt".to_string(), "eq".to_string(), "gt".to_string()]).is_err());
    }

    #[test]
    fn first_matching_cmp_matches_documented_example() {
        let base = vec!["10".to_string(), "5".to_string(), "small".to_string(), "10".to_string(), "middle".to_string(), "15".to_string(), "large".to_string(), "giant".to_string()];
        assert_eq!(call("first_matching_cmp", &base).unwrap(), "large");
        let mut sixteen = base.clone();
        sixteen[0] = "16".to_string();
        assert_eq!(call("first_matching_cmp", &sixteen).unwrap(), "giant");
        assert!(call("first_matching_cmp", &["1".to_string(), "x".to_string(), "y".to_string()]).is_err(), "odd argument count is a real error");
    }

    #[test]
    fn add_always_shows_a_trailing_point_zero_for_whole_results() {
        assert_eq!(call("add", &["2".to_string(), "3".to_string()]).unwrap(), "5.0");
        assert_eq!(call("add", &["2.5".to_string(), "0.5".to_string()]).unwrap(), "3.0");
        assert_eq!(call("add", &[]).unwrap(), "0", "zero arguments stays the bare Python int 0");
    }

    #[test]
    fn multiply_matches_python_int_vs_float_str_convention() {
        assert_eq!(call("multiply", &["2".to_string(), "3".to_string()]).unwrap(), "6.0");
        assert_eq!(call("multiply", &[]).unwrap(), "1");
    }

    #[test]
    fn subtract_and_divide_use_the_float_str_convention() {
        assert_eq!(call("subtract", &["5".to_string(), "3".to_string()]).unwrap(), "2.0");
        assert_eq!(call("divide", &["10".to_string(), "4".to_string()]).unwrap(), "2.5");
        assert_eq!(call("divide", &["10".to_string(), "2".to_string()]).unwrap(), "5.0");
        assert!(call("divide", &["1".to_string(), "0".to_string()]).is_err());
    }

    #[test]
    fn ceiling_floor_and_round_return_bare_integers() {
        assert_eq!(call("ceiling", &["4.1".to_string()]).unwrap(), "5");
        assert_eq!(call("floor", &["4.9".to_string()]).unwrap(), "4");
        assert_eq!(call("round", &["4.5".to_string()]).unwrap(), "4", "round-half-to-even: 4 is the nearest even integer");
        assert_eq!(call("round", &["5.5".to_string()]).unwrap(), "6", "round-half-to-even: 6 is the nearest even integer");
        assert_eq!(call("round", &["4.3".to_string()]).unwrap(), "4");
    }

    #[test]
    fn mod_follows_the_sign_of_the_divisor_like_python() {
        assert_eq!(call("mod", &["-7".to_string(), "3".to_string()]).unwrap(), "2", "Python: -7 % 3 == 2");
        assert_eq!(call("mod", &["7".to_string(), "-3".to_string()]).unwrap(), "-2", "Python: 7 % -3 == -2");
        assert!(call("mod", &["1".to_string(), "0".to_string()]).is_err());
    }

    #[test]
    fn fractional_part_keeps_the_sign_of_the_value() {
        assert_eq!(call("fractional_part", &["3.14".to_string()]).unwrap(), "0.14000000000000012", "matches Rust's f64 fract() representation of the same IEEE754 double Python's modf() operates on");
        assert_eq!(call("fractional_part", &["-3.5".to_string()]).unwrap(), "-0.5");
        assert_eq!(call("fractional_part", &["3".to_string()]).unwrap(), "0.0");
    }

    #[test]
    fn boolean_functions_match_documented_semantics() {
        assert_eq!(call("and", &["a".to_string(), "b".to_string()]).unwrap(), "1");
        assert_eq!(call("and", &["a".to_string(), "".to_string()]).unwrap(), "");
        assert_eq!(call("or", &["".to_string(), "b".to_string()]).unwrap(), "1");
        assert_eq!(call("or", &["".to_string(), "".to_string()]).unwrap(), "");
        assert_eq!(call("not", &["".to_string()]).unwrap(), "1");
        assert_eq!(call("not", &["x".to_string()]).unwrap(), "");
    }

    #[test]
    fn unknown_function_is_a_real_error() {
        assert!(call("no_such_function", &[]).is_err());
    }

    #[test]
    fn catalog_reports_correct_arity() {
        assert_eq!(arg_count("strcmp"), Some(Some(5)));
        assert_eq!(arg_count("add"), Some(None));
        assert_eq!(arg_count("not"), Some(Some(1)));
        assert_eq!(arg_count("no_such_function"), None);
    }
}
