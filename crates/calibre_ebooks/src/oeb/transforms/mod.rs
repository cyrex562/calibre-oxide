//! Port of `old_src/src/calibre/ebooks/oeb/transforms/`.
//!
//! These are the conversion-pipeline transforms the `Plumber` runs
//! against an in-memory [`crate::oeb::book::OEBBook`] -- a different,
//! simpler model than [`crate::oeb::polish`]'s dirty-tracking
//! `Container`/`Xml` (used by the separate "Polish Book" tool). There is
//! no on-disk container here: content is read/written as raw bytes
//! through `OEBBook::container` (the `oeb::container::Container` trait),
//! parsed on demand with [`crate::dom::Dom`] when a transform needs
//! to walk or mutate markup.
//!
//! Issue #41 batch 1 ported the 14 files with no dependency on the
//! other 6; batch 2 ports those remaining 6 (`embed_fonts.rs`,
//! `flatcss.rs`, `jacket.rs`, `rasterize.rs`, `split.rs`, `subset.rs`),
//! closing out the full `oeb/transforms/` directory.

pub mod alt_text;
pub mod cover;
pub mod data_url;
pub mod embed_fonts;
pub mod filenames;
pub mod flatcss;
pub mod guide;
pub mod htmltoc;
pub mod jacket;
pub mod linearize_tables;
pub mod manglecase;
pub mod metadata;
pub mod page_margin;
pub mod rasterize;
pub mod rescale;
pub mod split;
pub mod structure;
pub mod subset;
pub mod trimmanifest;
pub mod unsmarten;

/// Shared test-only fixture helpers: an in-memory [`Container`] and a
/// small builder for populating an [`OEBBook`] with manifest/spine
/// entries and raw content, so each transform's tests don't have to
/// redefine this (the pattern already used ad hoc in
/// `htmlz::oeb2html::tests`/`fb2::fb2ml::tests`).
#[cfg(test)]
pub(crate) mod test_support {
    use crate::oeb::book::OEBBook;
    use crate::oeb::container::Container;
    use crate::oeb::manifest::ManifestItem;
    use crate::oeb::spine::SpineItem;
    use anyhow::Result;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct MemContainer(HashMap<String, Vec<u8>>);

    impl Container for MemContainer {
        fn read(&self, path: &str) -> Result<Vec<u8>> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such part: {path}"))
        }
        fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
            self.0.insert(path.to_string(), data.to_vec());
            Ok(())
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn namelist(&self) -> Result<Vec<String>> {
            Ok(self.0.keys().cloned().collect())
        }
    }

    pub(crate) struct Builder {
        pub(crate) oeb: OEBBook,
        next: usize,
    }

    impl Builder {
        pub(crate) fn new() -> Self {
            let oeb = OEBBook::new(Box::new(MemContainer::default()));
            Self { oeb, next: 0 }
        }

        /// Add a manifest item with raw `content` at `href`, optionally
        /// in the spine.
        pub(crate) fn part(
            mut self,
            href: &str,
            media_type: &str,
            content: &[u8],
            in_spine: bool,
        ) -> Self {
            let id = format!("id{}", self.next);
            self.next += 1;
            self.oeb
                .manifest
                .items
                .insert(id.clone(), ManifestItem::new(&id, href, media_type));
            self.oeb.manifest.hrefs.insert(href.to_string(), id.clone());
            self.oeb.container.write(href, content).unwrap();
            if in_spine {
                self.oeb.spine.items.push(SpineItem::new(&id, true));
            }
            self
        }

        /// Add an XHTML spine page with `body` as its `<body>` content.
        pub(crate) fn page(self, href: &str, body: &str) -> Self {
            let content = format!(
                r#"<html xmlns="http://www.w3.org/1999/xhtml"><head></head><body>{body}</body></html>"#
            );
            self.part(href, "application/xhtml+xml", content.as_bytes(), true)
        }

        pub(crate) fn build(self) -> OEBBook {
            self.oeb
        }
    }
}
