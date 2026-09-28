//! Recipe input: not supported, and it says so.
//!
//! This used to write a page reading "Recipe Execution Not Supported" and
//! return it as the book. It is registered, so `ebook-convert my.recipe
//! out.epub` reported **success** and handed the user that page.
//!
//! A `.recipe` is a Python script: upstream ships ~1077 hand-written,
//! site-specific ones, and running any of them means executing arbitrary
//! Python. This build has no interpreter, so the original comment's reason
//! was correct.
//!
//! Worth recording, because I got this wrong once: the news *pipeline* is
//! fully ported ([`crate::web::feeds::download::build_index`], #81/#455),
//! which makes this look like wiring. It is not. `build_index` takes a
//! [`crate::web::feeds::recipe::RecipeConfig`] -- a declarative
//! description -- and the only thing that produces one is
//! `calibre_srv::news`, from an HTTP request. **Nothing turns a `.recipe`
//! file into a `RecipeConfig`**, because doing so means interpreting
//! Python. Fetching news works; converting a `.recipe` does not.
//!
//! So this refuses, and points at the mechanism that does work.

use crate::oeb::book::OEBBook;
use anyhow::{bail, Result};
use std::path::Path;

pub struct RecipeInput;

impl RecipeInput {
    pub fn new() -> Self {
        RecipeInput
    }

    pub fn convert(&self, input_path: &Path, _output_dir: &Path) -> Result<OEBBook> {
        bail!(
            "cannot convert {}: a .recipe is a Python script and this build has no interpreter to run it. \
             Fetching news itself is supported -- use the content server's news routes, which take the feed \
             list and options directly instead of a script",
            input_path.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_fails_instead_of_returning_a_fabricated_book() {
        let dir = tempfile::tempdir().unwrap();
        let recipe = dir.path().join("news.recipe");
        std::fs::write(&recipe, b"class MyRecipe(BasicNewsRecipe): pass").unwrap();
        let out = dir.path().join("out");

        let message = match RecipeInput::new().convert(&recipe, &out) {
            Ok(_) => panic!("an unimplemented format must not convert successfully"),
            Err(e) => format!("{e:#}"),
        };
        assert!(message.contains("Python"), "the error should say why: {message}");
        assert!(message.contains("news.recipe"), "the error should name the file: {message}");
        // And it points somewhere useful rather than dead-ending.
        assert!(message.contains("news routes"), "the error should point at what does work: {message}");
        assert!(!out.exists(), "nothing should have been written");
    }
}
