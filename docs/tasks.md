- [ ] Support downloading cover images (metadata fetching).
- [ ] **Device Detection**:
    - Port `calibre.devices` logic to detect USB storage and MTP devices.
    - specialized drivers for Kindle, Kobo, Nook, etc. [/]
- [ ] **Device View**:
    - A GUI view to show books currently on the connected device.
- [ ] **Transfer Logic**:
    - "Send to Device" functionality.
    - Automatic format selection (e.g., convert EPUB to MOBI/AZW3 for Kindle).
    - Metadata updating on the device (updating `metadata.calibre` on the device).

- [ ] **Phase 7: Conversion Pipeline**:
    - [x] **Enhance `EpubInput` Plugin**:
        - [x] Unzip EPUB to a temporary directory.
        - [x] Parse `content.opf` to populate `OebBook.manifest` (all resources).
        - [x] Parse `content.opf` to populate `OebBook.spine` (reading order).
        - [x] Extract `guide` (references) and `toc` (Table of Contents). (Partially done via OPF)
    - [ ] **Implement `EpubOutput` Plugin**:
        - [ ] Create `calibre_conversion/src/plugins/epub_output.rs`.
        - [ ] Implement `write` method to zip `OebBook` contents into a valid EPUB.
        - [ ] Generate `content.opf` from `OebBook` metadata and manifest.
        - [ ] Generate `toc.ncx` and `nav.xhtml`.
        - [ ] Ensure correct `mimetype` and `container.xml` placement.
    - [ ] **Core Processing (The Pipeline)**:
        - [ ] Implement HTML processing using `html5ever` (cleanup, normalization).
        - [ ] Implement CSS flattening/processing (handling local vs external styles).
        - [ ] Image processing (resizing, format conversion).
    - [ ] **CLI Tool**:
        - [ ] Create `src/bin/ebook-convert.rs`.
        - [ ] CLI args: `input_file`, `output_file`, `--profile`.
        - [ ] Wire up `ConversionPipeline` to the CLI entry point.




select the next set of files to port to rust, port them to rust, then mark them complete in modules_to_port.md. If you decide to create a stub mark it as such in docs/stubs_to_complete.md. Ensure that each ported module has passing property and/or unit tests.