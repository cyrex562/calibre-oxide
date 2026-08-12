//! APNX (Amazon Page Number Index) page-generation domain model.
//!
//! Port of `old_src/src/calibre/devices/kindle/apnx_page_generator/`.
//! Covers `page_number_type.py`, `page_group.py`, `pages.py`, and
//! `i_page_generator.py`. Individual concrete generators
//! (Fast/Accurate/Exact/Pagebreak) live under `generators/` and are a
//! separate issue.

pub mod i_page_generator;
pub mod page_group;
pub mod page_number_type;
pub mod pages;

pub use i_page_generator::{mobi_html_length, IPageGenerator};
pub use page_group::PageGroup;
pub use page_number_type::PageNumberType;
pub use pages::Pages;
