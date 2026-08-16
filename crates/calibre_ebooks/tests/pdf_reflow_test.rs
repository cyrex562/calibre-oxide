//! Integration tests for `calibre_ebooks::pdf` (issue #45), exercising the
//! public API from outside the crate the way a real caller would.

use calibre_ebooks::pdf::pdftohtml::{find_pdftohtml, pdftohtml, PdfToHtmlError};
use calibre_ebooks::pdf::reflow::{PdfDocument, ReflowLog, ReflowOpts};
use calibre_ebooks::pdf::utils::encode_for_xml;

fn fixture_xml() -> &'static str {
    r##"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml>
<page number="1" position="absolute" top="0" left="0" height="792" width="612">
<fontspec id="0" size="10" family="Times" color="#000000"/>
<fontspec id="1" size="18" family="Times" color="#000000"/>
<text top="40" left="180" width="250" height="24" font="1">Chapter One: Beginnings</text>
<text top="100" left="72" width="470" height="14" font="0">It was the best of times as far as synthetic PDF fixtures go, a line long enough to</text>
<text top="115" left="72" width="200" height="14" font="0">clear the unwrap-factor threshold easily.</text>
</page>
</pdf2xml>"##
}

#[test]
fn pdf_document_from_xml_reconstructs_a_heading_and_a_paragraph() {
    let mut opts = ReflowOpts {
        pdf_header_skip: 0.0,
        pdf_footer_skip: 0.0,
        ..ReflowOpts::default()
    };
    let mut log = ReflowLog::default();
    let mut doc = PdfDocument::from_xml(fixture_xml(), &mut opts, &mut log).expect("valid fixture");

    assert_eq!(doc.pages.len(), 1);
    let page = &doc.pages[0];

    let heading = page
        .texts
        .iter()
        .find(|t| t.text_as_string.contains("Chapter One"))
        .expect("heading survives reflow");
    assert_eq!(
        heading.tag, "h2",
        "chapter heading should be promoted to <h2>"
    );

    let body = page
        .texts
        .iter()
        .find(|t| t.text_as_string.contains("best of times"))
        .expect("body paragraph survives reflow");
    assert!(
        body.text_as_string.contains("unwrap-factor threshold"),
        "wrapped second line should have been coalesced into the same paragraph, got: {:?}",
        body.text_as_string
    );

    let html = doc.render_html("Integration Test");
    assert!(html.contains("<h2"));
    assert!(html.contains("Chapter One"));
    assert!(html.contains("</html>"));
}

#[test]
fn pdf_document_from_xml_rejects_malformed_xml_without_panicking() {
    let mut opts = ReflowOpts::default();
    let mut log = ReflowLog::default();
    let result = PdfDocument::from_xml("not xml at all <<<", &mut opts, &mut log);
    assert!(result.is_err());
}

#[test]
fn pdf_document_from_xml_rejects_text_with_unknown_font_id() {
    let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml>
<page number="1" position="absolute" top="0" left="0" height="792" width="612">
<fontspec id="0" size="10" family="Times" color="#000000"/>
<text top="10" left="10" width="50" height="12" font="99">orphaned font reference</text>
</page>
</pdf2xml>"##;
    let mut opts = ReflowOpts::default();
    let mut log = ReflowLog::default();
    let result = PdfDocument::from_xml(xml, &mut opts, &mut log);
    assert!(
        result.is_err(),
        "unknown font id should be a graceful error, not a panic"
    );
}

#[test]
fn pdftohtml_binary_not_found_returns_a_clear_error() {
    // Redirect PATH to a directory that can't possibly contain pdftohtml,
    // for just this one call.
    let old_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("PATH", "");
    }
    let tmp_in = tempfile::tempdir().expect("tempdir");
    let fake_pdf = tmp_in.path().join("in.pdf");
    std::fs::write(&fake_pdf, b"%PDF-1.4\n").expect("write fake pdf");
    let out_dir = tmp_in.path().join("out");
    let result = pdftohtml(&out_dir, &fake_pdf, false, true);
    unsafe {
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }
    assert!(matches!(result, Err(PdfToHtmlError::BinaryNotFound(_))));
}

#[test]
fn pdftohtml_real_invocation_if_poppler_is_installed() {
    // Opportunistic per docs/HARNESS.md: skip gracefully if pdftohtml
    // isn't installed in this environment rather than failing the suite.
    let Some(_binary) = find_pdftohtml() else {
        eprintln!("pdftohtml not found on PATH; skipping real-invocation assertions");
        return;
    };
    // We don't have a real PDF fixture on hand in this test crate, so just
    // confirm invoking against a bogus PDF fails cleanly (non-zero exit,
    // surfaced as an Err, not a panic) rather than asserting success.
    let tmp_in = tempfile::tempdir().expect("tempdir");
    let fake_pdf = tmp_in.path().join("not-a-real.pdf");
    std::fs::write(&fake_pdf, b"this is not a pdf").expect("write fake pdf");
    let out_dir = tmp_in.path().join("out");
    let result = pdftohtml(&out_dir, &fake_pdf, false, true);
    assert!(
        result.is_err(),
        "pdftohtml should refuse to process a non-PDF file"
    );
}

#[test]
fn encode_for_xml_is_reachable_from_outside_the_crate() {
    assert_eq!(encode_for_xml("<a & b>"), "&lt;a &amp; b&gt;");
}
