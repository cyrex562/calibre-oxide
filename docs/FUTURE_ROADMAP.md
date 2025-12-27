# Future Roadmap: Calibre-Oxide Functional Parity

This document outlines the remaining work required to make `calibre-oxide` a fully functional clone of the legacy Calibre application.

## Phase 5: Library Management (Write Operations)
*Current Status: Partially Complete*

- [x] **Database Write Support**:
    - [x] Implement `INSERT`, `UPDATE`, `DELETE` operations in `calibre_db`.
    - [x] Handle transaction safety and `metadata.db` schema consistency.
- [/] **File Management**:
    - [ ] Logic to move/rename book files on disk when metadata changes (Author/Title).
    - [x] Logic to import new files (copy to library folder, create DB records).
- [x] **Metadata Editing GUI**:
    - [x] Create an "Edit Metadata" dialog in Iced.
    - [x] Two-way binding between GUI forms and `calibre_db`.
- [/] **Cover Management**:
    - [x] Render book covers in the `BookList` view (optimizing for performance).
    - [ ] Support replacing/downloading cover images.

## Phase 6: Device Integration
*Current Status: Not Started*

- [ ] **Device Detection**:
    - Port `calibre.devices` logic to detect USB storage and MTP devices.
    - specialized drivers for Kindle, Kobo, Nook, etc.
- [ ] **Device View**:
    - A GUI view to show books currently on the connected device.
- [ ] **Transfer Logic**:
    - "Send to Device" functionality.
    - Automatic format selection (e.g., convert EPUB to MOBI/AZW3 for Kindle).
    - Metadata updating on the device (updating `metadata.calibre` on the device).

## Phase 7: Conversion Engine
*Current Status: Read-Only Metadata Parsing*

- [ ] **Conversion Pipeline**:
    - Port the massive `calibre.ebooks.conversion` module. This is the most complex task.
    - Implement Input Plugins (EPUB, MOBI, PDF, HTML, TXT, etc.).
    - Implement Output Plugins.
    - Implement the aesthetic processing pipeline (CSS flattening, font subsetting, etc.).
- [ ] **Queuing System**:
    - Background job manager for long-running conversions.

## Phase 8: E-book Viewer
*Current Status: Not Started*

- [ ] **Internal Viewer**:
    - Integrate a web view (e.g., `wry` or `tauri`) or building a native renderer for EPUB/HTML.
    - Support for user CSS, bookmarks, and highlights.

## Phase 9: Advanced Features & Polish

- [ ] **Virtual Libraries**: Tabs/Views filtering by SQL queries.
- [ ] **Tag Browser**: Interactive tree view of Authors, Series, Tags.
- [ ] **Content Server**: Embedded HTTP server to host the library.
- [ ] **Plugins**: Logic to load external code (likely WebAssembly or scripting) to extend functionality.

## Immediate Next Steps (Recommended)
1. **Cover Rendering**: It makes the app feel "real".
2. **Add Book**: The most essential library function.
3. **Edit Metadata**: To fix import errors.
