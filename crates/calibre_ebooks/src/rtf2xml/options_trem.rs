//! Port of `old_src/src/calibre/ebooks/rtf2xml/options_trem.py`
//! (`ParseOptions`).
//!
//! A small, hand-rolled `getopt`-like command-line option parser,
//! independent of any RTF-specific logic. Given a spec of legal
//! options (each with whether it takes an argument, and an optional
//! short-flag letter) and the raw argument list, [`OptionParser::parse`]
//! substitutes short flags with their long form, pairs `--opt value`
//! into `--opt=value`, splits the run into "options" (everything up to
//! and including the last flag-looking token) vs. "positional
//! arguments" (everything after), validates that every option is
//! recognized, and returns the parsed `--opt=value` pairs as a dict
//! alongside the leftover positional arguments.
//!
//! Every validation failure along the way (an unrecognized option, an
//! argument-taking option missing its value, a stray positional
//! argument mixed into the option run) prints the same diagnostic
//! Python does and flips an internal flag; [`OptionParser::parse`]
//! returns `None` if that flag ever got flipped, matching Python's own
//! `(0, 0)` sentinel-pair return.
//!
//! # A crash averted
//!
//! `__make_options_dict`'s `option, arg = item.split('=')` (no
//! `maxsplit`) requires *exactly* two pieces -- an argument value
//! containing its own `=` (e.g. `--config=key=value`) raises an
//! uncaught `ValueError` in Python. This port uses `split_once('=')`
//! instead, keeping everything after the first `=` as part of the
//! value, per this crate's standing convention of not introducing a
//! panic equivalent to an upstream crash for realistic input.

use std::collections::HashSet;

use indexmap::IndexMap;

/// One option's spec: whether it takes an argument, and its optional
/// short-flag letter. Unlike Python's `[takes_arg, short]` two-element
/// list (which can be zero, one, or two items long, each access
/// guarded by its own `try/except IndexError`), this is a fixed-shape
/// struct -- a caller who wants "no short flag" passes `None`, which
/// behaves identically to Python's "list too short to have a second
/// item" case.
#[derive(Debug, Clone, Copy)]
pub struct OptionSpec {
    pub takes_arg: bool,
    pub short: Option<char>,
}

pub struct OptionParser {
    legal_options: HashSet<String>,
    short_long: IndexMap<String, String>,
    opt_with_args: HashSet<String>,
}

impl OptionParser {
    /// Port of `ParseOptions.__init__`'s three `__make_*` helper
    /// calls.
    pub fn new(options: &IndexMap<String, OptionSpec>) -> Self {
        let mut legal_options = HashSet::new();
        let mut short_long = IndexMap::new();
        let mut opt_with_args = HashSet::new();
        for (key, spec) in options {
            let long = format!("--{key}");
            legal_options.insert(long.clone());
            if let Some(short_char) = spec.short {
                let short = format!("-{short_char}");
                legal_options.insert(short.clone());
                short_long.insert(short, long.clone());
            }
            if spec.takes_arg {
                opt_with_args.insert(long);
            }
        }
        Self { legal_options, short_long, opt_with_args }
    }

    /// Port of `__sub_short_with_long`.
    fn sub_short_with_long(&self, system_string: &[String]) -> Vec<String> {
        system_string.iter().map(|item| self.short_long.get(item).cloned().unwrap_or_else(|| item.clone())).collect()
    }

    /// Port of `__pair_arg_with_option`.
    fn pair_arg_with_option(&self, system_string: &[String], options_okay: &mut bool) -> Vec<String> {
        let opt_len = system_string.len();
        let mut new_system_string = Vec::new();
        let mut counter = 0usize;
        let mut slurp_value = false;
        for arg in system_string {
            counter += 1;
            if slurp_value {
                slurp_value = false;
                continue;
            }
            if !arg.starts_with('-') {
                new_system_string.push(arg.clone());
            } else if arg.contains('=') {
                new_system_string.push(arg.clone());
            } else if self.opt_with_args.contains(arg) {
                if counter + 1 > opt_len {
                    eprintln!("option \"{arg}\" must take an argument");
                    new_system_string.push(arg.clone());
                    *options_okay = false;
                } else if system_string[counter].starts_with('-') {
                    eprintln!("option \"{arg}\" must take an argument");
                    new_system_string.push(arg.clone());
                    *options_okay = false;
                } else {
                    new_system_string.push(format!("{arg}={}", system_string[counter]));
                    slurp_value = true;
                }
            } else {
                new_system_string.push(arg.clone());
            }
        }
        new_system_string
    }

    /// Port of `__get_just_options`.
    fn get_just_options(system_string: &[String], options_okay: &mut bool) -> (Vec<String>, Vec<String>) {
        let mut highest = 0usize;
        let mut counter = 0usize;
        let mut found_options = false;
        for item in system_string {
            if item.starts_with('-') {
                highest = counter;
                found_options = true;
            }
            counter += 1;
        }
        let (just_options, arguments) = if found_options {
            (system_string[..=highest].to_vec(), system_string[highest + 1..].to_vec())
        } else {
            (Vec::new(), system_string.to_vec())
        };
        if found_options {
            for item in &just_options {
                if !item.starts_with('-') {
                    eprintln!("{item} is an argument in an option list");
                    *options_okay = false;
                }
            }
        }
        (just_options, arguments)
    }

    /// Port of `__is_legal_option_func`.
    fn is_legal_option(&self, system_string: &[String], options_okay: &mut bool) {
        let mut illegal = Vec::new();
        for arg in system_string {
            let bare = arg.split('=').next().unwrap_or(arg.as_str());
            if !self.legal_options.contains(bare) && bare.starts_with('-') {
                illegal.push(bare.to_string());
            }
        }
        if !illegal.is_empty() {
            *options_okay = false;
            eprintln!("The following options are not permitted:");
            for item in &illegal {
                eprintln!("{item}");
            }
        }
    }

    /// Port of `__make_options_dict` -- see this module's own doc for
    /// why an argument value containing `=` doesn't panic here.
    fn make_options_dict(options: &[String]) -> IndexMap<String, Option<String>> {
        let mut dict = IndexMap::new();
        for item in options {
            let (mut option, arg) = match item.split_once('=') {
                Some((o, a)) => (o.to_string(), Some(a.to_string())),
                None => (item.clone(), None),
            };
            if let Some(stripped) = option.strip_prefix('-') {
                option = stripped.to_string();
            }
            if let Some(stripped) = option.strip_prefix('-') {
                option = stripped.to_string();
            }
            dict.insert(option, arg);
        }
        dict
    }

    /// Port of `ParseOptions.parse_options`. `args` should already
    /// exclude the program name (Python's own constructor does
    /// `system_string[1:]`, i.e. `sys.argv` minus `argv[0]`).
    /// Returns `None` for Python's `(0, 0)` failure sentinel.
    pub fn parse(&self, args: &[String]) -> Option<(IndexMap<String, Option<String>>, Vec<String>)> {
        let mut options_okay = true;
        let subbed = self.sub_short_with_long(args);
        let paired = self.pair_arg_with_option(&subbed, &mut options_okay);
        let (just_options, arguments) = Self::get_just_options(&paired, &mut options_okay);
        self.is_legal_option(&paired, &mut options_okay);
        if options_okay {
            Some((Self::make_options_dict(&just_options), arguments))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> IndexMap<String, OptionSpec> {
        let mut m = IndexMap::new();
        m.insert("indents".to_string(), OptionSpec { takes_arg: false, short: Some('i') });
        m.insert("output".to_string(), OptionSpec { takes_arg: true, short: Some('o') });
        m
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_long_option_with_an_inline_equals_argument() {
        let parser = OptionParser::new(&spec());
        let (options, arguments) = parser.parse(&args(&["--output=file.txt"])).unwrap();
        assert_eq!(options.get("output"), Some(&Some("file.txt".to_string())));
        assert!(arguments.is_empty());
    }

    #[test]
    fn a_long_option_paired_with_a_following_token_as_its_argument() {
        let parser = OptionParser::new(&spec());
        let (options, arguments) = parser.parse(&args(&["--output", "file.txt"])).unwrap();
        assert_eq!(options.get("output"), Some(&Some("file.txt".to_string())));
        assert!(arguments.is_empty());
    }

    #[test]
    fn a_short_option_is_substituted_with_its_long_form_first() {
        let parser = OptionParser::new(&spec());
        let (options, arguments) = parser.parse(&args(&["-o", "file.txt"])).unwrap();
        assert_eq!(options.get("output"), Some(&Some("file.txt".to_string())));
        assert!(arguments.is_empty());
    }

    #[test]
    fn a_boolean_flag_takes_no_argument() {
        let parser = OptionParser::new(&spec());
        let (options, arguments) = parser.parse(&args(&["--indents"])).unwrap();
        assert_eq!(options.get("indents"), Some(&None));
        assert!(arguments.is_empty());
    }

    #[test]
    fn positional_arguments_after_the_last_option_are_separated_out() {
        let parser = OptionParser::new(&spec());
        let (options, arguments) = parser.parse(&args(&["--indents", "input.txt"])).unwrap();
        assert_eq!(options.get("indents"), Some(&None));
        assert_eq!(arguments, vec!["input.txt".to_string()]);
    }

    #[test]
    fn an_unrecognized_option_fails_parsing() {
        let parser = OptionParser::new(&spec());
        assert!(parser.parse(&args(&["--nonexistent"])).is_none());
    }

    #[test]
    fn an_argument_taking_option_with_no_following_token_fails_parsing() {
        let parser = OptionParser::new(&spec());
        assert!(parser.parse(&args(&["--output"])).is_none());
    }

    #[test]
    fn an_argument_taking_option_followed_by_another_option_fails_parsing() {
        let parser = OptionParser::new(&spec());
        assert!(parser.parse(&args(&["--output", "--indents"])).is_none());
    }

    #[test]
    fn a_positional_argument_mixed_into_the_option_run_fails_parsing() {
        let parser = OptionParser::new(&spec());
        assert!(parser.parse(&args(&["input.txt", "--indents"])).is_none());
    }

    #[test]
    fn no_arguments_at_all_yields_empty_options_and_arguments() {
        let parser = OptionParser::new(&spec());
        let (options, arguments) = parser.parse(&args(&[])).unwrap();
        assert!(options.is_empty());
        assert!(arguments.is_empty());
    }
}
