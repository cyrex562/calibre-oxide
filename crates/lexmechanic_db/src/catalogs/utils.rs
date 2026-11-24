//! Port of `old_src/src/calibre/library/catalogs/utils.py`'s
//! `NumberToText` -- a recursive number-to-English-words converter (`4.56`
//! -> `"four point fifty-six"`, `456` -> `"four hundred fifty-six"`, `4:56`
//! -> `"four fifty-six"`).
//!
//! Ported for API completeness/issue #57 parity, but its one call site
//! upstream (`epub_mobi_builder.py`'s `generate_sort_title`) is inside an
//! `if False:` block guarded by a `numbers_as_text` option no longer wired
//! to anything, with the comment "*** Keep this code in case we need to
//! restore numbers_as_text ***" -- `NumberToText` is dead code in the
//! *current* upstream source, not reachable from any live code path.
//!
//! # What's not here
//!
//! Upstream's `NumberToText` is a class exposing `.text`, `.number_as_float`,
//! and `.suffix` attributes, plus a `verbose`/`self.log` debug-print mode.
//! `number_as_float` and `suffix` are write-only in the Python source --
//! grepped across every file in this crate's `catalogs` module, nothing
//! ever reads either back -- so only `.text` (this function's return value)
//! is ported. The debug-log mode is likewise dropped, matching this
//! crate's general convention of not porting `verbose`/debug-print
//! plumbing that has no effect on a pass's actual output.
//!
//! # Preserved upstream bugs
//!
//! This function is fragile by construction upstream, and several of its
//! rough edges are preserved verbatim rather than fixed, since its one
//! real caller only ever feeds it short, mostly-numeric tokens (genre/
//! series-index text) where these edges rarely bite, and "fixing" them
//! would silently change output for any token they *do* affect:
//!
//! - **Ordinal detection is a character class, not a suffix match.**
//!   `re.search(r'[st|nd|rd|th]', ...)` is a *character class* containing
//!   `s`, `t`, `n`, `d`, `r`, `h`, `|` (regex alternation doesn't apply
//!   inside `[...]`), not a match for the literal suffixes `st`/`nd`/`rd`/
//!   `th`. Any input containing even one of those characters anywhere
//!   takes the ordinal branch, not just genuine ordinals like `"2nd"`.
//! - **The ordinal branch swallows non-digit suffixes for numbers > 9.**
//!   When the digit-only remainder exceeds 9, the branch recurses on just
//!   the digits and discards everything else (e.g. `"50 percent"` ->
//!   `"fifty"`, not `"fifty percent"` -- the percent branch delegates to
//!   the ordinal branch here since `"percent"` contains `t`/`n`).
//! - **Hyphenated tokens translate only the numeric side.** `"20-and"`
//!   translates the left side and keeps `"and"` raw; `"and-20"` keeps
//!   `"and"` raw and translates the right side -- neither side is translated
//!   if both look non-numeric to the branch's own `\d` check on the left
//!   half only.
//!
//! Cases that would raise an *uncaught* Python exception -- `int('')` on an
//! ordinal with no digits, or `.start()` on a `None` regex match when a
//! "hybrid" token has no digit at all -- return an empty string here
//! instead of panicking, matching this crate's standing convention for
//! untrusted/malformed input.
//!
//! The class's own docstring examples (`"4:56 => four fifty-six"`) are
//! informal and don't match what the code actually produces -- tracing the
//! real control flow, both the time and decimal branches join two
//! independently-`.text`-ed (and thus independently self-capitalized)
//! recursive results, so e.g. `"4:56"` actually produces `"Four-Fifty-six"`.
//! This port follows the traced behavior, not the docstring.

const ORDINALS: [&str; 10] = [
    "zeroth", "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
];
const LESS_THAN_TWENTY: [&str; 20] = [
    "<zero>", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "<zero>", "<tens>", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty",
    "ninety",
];
const HUNDREDS: [&str; 10] = [
    "<zero>", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

/// Port of Python's `str.capitalize()`: uppercase the first character,
/// lowercase the rest (not merely uppercase-first, which `str.title()`-style
/// helpers often mean instead).
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
        }
        None => String::new(),
    }
}

/// Port of `NumberToText.stringFromInt`: render a three-digit (0-999)
/// number as words.
fn string_from_int(int_to_translate: u32) -> String {
    let hundreds_component = int_to_translate - (int_to_translate % 100);
    let tens_component = int_to_translate % 100;

    let hundreds_component_string = if hundreds_component > 0 {
        format!("{} hundred", HUNDREDS[(hundreds_component / 100) as usize])
    } else {
        String::new()
    };

    let tens_component_string = if tens_component < 20 {
        LESS_THAN_TWENTY[tens_component as usize].to_string()
    } else {
        let tens_part = TENS[(tens_component / 10) as usize];
        let ones_part = LESS_THAN_TWENTY[(tens_component % 10) as usize];
        if int_to_translate % 10 != 0 {
            format!("{tens_part}-{ones_part}")
        } else {
            tens_part.to_string()
        }
    };

    if hundreds_component > 0 && tens_component == 0 {
        hundreds_component_string
    } else if hundreds_component == 0 && tens_component > 0 {
        tens_component_string
    } else if hundreds_component > 0 && tens_component > 0 {
        format!("{hundreds_component_string} {tens_component_string}")
    } else {
        String::new()
    }
}

/// Port of `NumberToText(number).text` -- translate `number` to English
/// words, returning `""` where upstream would leave `self.text` at its
/// default (or raise, per this module's doc).
pub fn number_to_text(number: &str) -> String {
    // Ordinal: any of s/t/n/d/r/h/| appears anywhere (see module doc).
    if number.contains(['s', 't', 'n', 'd', 'r', 'h', '|']) {
        let cleaned = number.replace(',', "");
        let Some(suffix_start) = cleaned.find(|c: char| !c.is_ascii_digit()) else {
            return String::new();
        };
        let ordinal_number: String = cleaned.chars().filter(|c| c.is_ascii_digit()).collect();
        let Ok(n) = ordinal_number.parse::<u64>() else {
            return String::new();
        };
        let _suffix = &cleaned[suffix_start..];
        return if n > 9 {
            number_to_text(&ordinal_number)
        } else {
            ORDINALS[n as usize].to_string()
        };
    }

    // Time. Splits on every ':' (not just the first) like Python's
    // `.split(':')`, but only the first two parts are ever read -- a
    // third `"1:2:3"`-style segment is silently dropped, matching
    // upstream's own `time_strings[0]`/`time_strings[1]` indexing.
    if number.contains(':') {
        let parts: Vec<&str> = number.split(':').collect();
        let hours = number_to_text(parts[0]);
        let minutes = number_to_text(parts.get(1).copied().unwrap_or(""));
        return format!("{}-{}", capitalize(&hours), minutes);
    }

    // Percent.
    if number.contains('%') {
        return number_to_text(&number.replace('%', " percent"));
    }

    // Decimal. Same full-split-but-only-first-two-parts-read shape as
    // the time branch above (`"5.5.5"` -> point-composes just `"5"` and
    // `"5"`, dropping the third segment).
    if number.contains('.') {
        let parts: Vec<&str> = number.split('.').collect();
        let left = number_to_text(parts[0]);
        let right = number_to_text(parts.get(1).copied().unwrap_or(""));
        return format!("{} point {}", capitalize(&left), right);
    }

    // Hyphenated. Same full-split shape again.
    if number.contains('-') {
        let strings: Vec<&str> = number.split('-').collect();
        let (left, right) = if strings[0].contains(|c: char| c.is_ascii_digit()) {
            (
                number_to_text(strings[0]),
                strings.get(1).copied().unwrap_or("").to_string(),
            )
        } else {
            (
                strings[0].to_string(),
                number_to_text(strings.get(1).copied().unwrap_or("")),
            )
        };
        return format!("{left}-{right}");
    }

    // Comma-grouped digits only.
    if number.contains(',') && number.chars().all(|c| c.is_ascii_digit() || c == ',') {
        return number_to_text(&number.replace(',', ""));
    }

    // Hybrid: contains at least one non-digit character.
    if number.chars().any(|c| !c.is_ascii_digit()) {
        let Some(text_position) = number.find(|c: char| !c.is_ascii_digit()) else {
            return String::new();
        };
        return match number.find(|c: char| c.is_ascii_digit()) {
            Some(number_position) if number_position < text_position => {
                let head = &number[..text_position];
                let tail = &number[text_position..];
                format!("{}{}", number_to_text(head), tail)
            }
            Some(number_position) => {
                let head = &number[..number_position];
                let tail = &number[number_position..];
                format!("{}{}", head, number_to_text(tail))
            }
            None => String::new(),
        };
    }

    // Clean: a plain (possibly empty) run of digits.
    let Ok(n) = number.parse::<i64>() else {
        return String::new();
    };
    if n > 1_000_000_000 {
        return format!("{n} out of range");
    }
    if n == 1_000_000_000 {
        return "one billion".to_string();
    }

    let millions = (n / 1_000_000) as u32;
    let thousands = ((n - i64::from(millions) * 1_000_000) / 1_000) as u32;
    let hundreds = (n - i64::from(millions) * 1_000_000 - i64::from(thousands) * 1_000) as u32;

    if thousands > 0 && n > 1099 && n < 2000 {
        let result = format!(
            "{} {}",
            LESS_THAN_TWENTY[(n / 100) as usize],
            string_from_int((n % 100) as u32)
        );
        return capitalize(result.trim());
    }

    let hundreds_string = if hundreds > 0 {
        string_from_int(hundreds)
    } else {
        String::new()
    };
    let thousands_string = if thousands > 0 {
        string_from_int(thousands)
    } else {
        String::new()
    };
    let millions_string = if millions > 0 {
        string_from_int(millions)
    } else {
        String::new()
    };

    let mut result = String::new();
    if millions > 0 {
        result.push_str(&format!("{millions_string} million "));
    }
    if thousands > 0 {
        result.push_str(&format!("{thousands_string} thousand "));
    }
    if hundreds > 0 {
        result.push_str(&hundreds_string);
    }
    if millions == 0 && thousands == 0 && hundreds == 0 {
        result = "zero".to_string();
    }
    capitalize(result.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_three_digit_number() {
        assert_eq!(number_to_text("456"), "Four hundred fifty-six");
    }

    #[test]
    fn decimal_point() {
        // Each side is independently translated (and self-capitalized) --
        // the format string doesn't re-lowercase the right-hand side, so
        // this reads "Four point Fifty-six", not the docstring's informal
        // "four point fifty-six".
        assert_eq!(number_to_text("4.56"), "Four point Fifty-six");
    }

    #[test]
    fn time_of_day() {
        // Same story: both sides keep their own capitalization, joined by
        // a literal hyphen -- "Four-Fifty-six", not the docstring's
        // informal "four fifty-six".
        assert_eq!(number_to_text("4:56"), "Four-Fifty-six");
    }

    #[test]
    fn zero_is_zero() {
        assert_eq!(number_to_text("0"), "Zero");
    }

    #[test]
    fn one_billion_is_a_special_case() {
        assert_eq!(number_to_text("1000000000"), "one billion");
    }

    #[test]
    fn over_one_billion_reports_out_of_range() {
        assert_eq!(number_to_text("2000000000"), "2000000000 out of range");
    }

    #[test]
    fn year_style_thousands_1100_to_1999() {
        assert_eq!(number_to_text("1984"), "Nineteen eighty-four");
    }

    #[test]
    fn thousands_outside_the_year_range_use_the_long_form() {
        assert_eq!(number_to_text("3000"), "Three thousand");
    }

    #[test]
    fn millions() {
        assert_eq!(number_to_text("2500000"), "Two million five hundred thousand");
    }

    #[test]
    fn ordinal_first() {
        assert_eq!(number_to_text("1st"), "first");
    }

    #[test]
    fn ordinal_beyond_nine_recurses_and_drops_the_suffix() {
        // Preserved upstream bug: the ordinal branch discards the
        // non-digit suffix once the digit run exceeds 9.
        assert_eq!(number_to_text("22nd"), "Twenty-two");
    }

    #[test]
    fn comma_grouped_digits() {
        assert_eq!(number_to_text("4,560"), "Four thousand five hundred sixty");
    }

    #[test]
    fn hyphenated_translates_only_the_numeric_side() {
        // "family" avoids every ordinal-trigger character (s/t/n/d/r/h) so
        // it actually reaches the hyphenated branch instead of being
        // swept into the ordinal branch first (as e.g. "and" would be,
        // since it contains 'n' and 'd').
        assert_eq!(number_to_text("20-family"), "Twenty-family");
        assert_eq!(number_to_text("family-20"), "family-Twenty");
    }

    #[test]
    fn empty_input_yields_empty_text() {
        assert_eq!(number_to_text(""), "");
    }

    #[test]
    fn pure_letters_with_no_ordinal_characters_do_not_panic() {
        assert_eq!(number_to_text("aeiou"), "");
    }

    #[test]
    fn pure_letters_hitting_the_ordinal_branch_do_not_panic() {
        // Contains 't' and 'n' -> takes the (buggy) ordinal branch upstream.
        assert_eq!(number_to_text("stand"), "");
    }

    // The following were cross-validated against a direct run of the real
    // upstream Python class (with its `calibre`-internal `prints`/`log`
    // imports stubbed out) to catch subtle mistranslations a hand trace
    // could miss.

    #[test]
    fn year_style_lower_bound_produces_an_incomplete_result() {
        // A genuine upstream bug: 1100's "hundreds" remainder is exactly
        // 0, and `stringFromInt(0)` returns "" (neither of its two
        // truthy-component branches fire), so the year-style format
        // string's second half silently vanishes.
        assert_eq!(number_to_text("1100"), "Eleven");
    }

    #[test]
    fn year_style_upper_bound_is_exclusive() {
        assert_eq!(number_to_text("2000"), "Two thousand");
    }

    #[test]
    fn year_style_lower_bound_is_exclusive() {
        assert_eq!(number_to_text("1099"), "One thousand ninety-nine");
        assert_eq!(number_to_text("1101"), "Eleven one");
    }

    #[test]
    fn multi_delimiter_time_only_reads_the_first_two_segments() {
        assert_eq!(number_to_text("1:2:3"), "One-Two");
    }

    #[test]
    fn multi_delimiter_decimal_only_reads_the_first_two_segments() {
        assert_eq!(number_to_text("5.5.5"), "Five point Five");
    }

    #[test]
    fn leading_hyphen_treats_the_empty_left_side_as_non_numeric() {
        assert_eq!(number_to_text("-5"), "-Five");
    }

    #[test]
    fn trailing_hyphen_leaves_the_empty_right_side_untranslated() {
        assert_eq!(number_to_text("5-"), "Five-");
    }

    #[test]
    fn hybrid_ordering_depends_on_which_class_of_character_appears_first() {
        assert_eq!(number_to_text("abc123"), "abcOne hundred twenty-three");
        assert_eq!(number_to_text("123abc"), "One hundred twenty-threeabc");
    }

    #[test]
    fn multi_group_comma_separated_thousands() {
        assert_eq!(
            number_to_text("1,234,567"),
            "One million two hundred thirty-four thousand five hundred sixty-seven"
        );
    }

    #[test]
    fn tenth_is_not_treated_as_a_single_digit_ordinal() {
        // "10th" has digit run "10" (> 9), so it recurses into the plain
        // number path rather than indexing straight into `ORDINALS`.
        assert_eq!(number_to_text("10th"), "Ten");
    }
}
