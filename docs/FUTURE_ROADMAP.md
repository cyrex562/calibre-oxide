# Future Roadmap: Calibre-Oxide Functional Parity

This document outlines the remaining work required to make `calibre-oxide` a fully functional clone of the legacy Calibre application.

## Phase 5: Library Management (Write Operations)
*Current Status: Partially Complete*

- [x] **Database Write Support**:
    - [x] Implement `INSERT`, `UPDATE`, `DELETE` operations in `calibre_db`.
    - [x] Handle transaction safety and `metadata.db` schema consistency.
- [/] **File Management**:
    - [X] Logic to move/rename book files on disk when metadata changes (Author/Title).
    - [x] Logic to import new files (copy to library folder, create DB records).
- [x] **Metadata Editing GUI**:
    - [x] Create an "Edit Metadata" dialog in Iced.
    - [x] Two-way binding between GUI forms and `calibre_db`.
- [/] **Cover Management**:
    - [x] Render book covers in the `BookList` view (optimizing for performance).
    - [x] Support replacing cover images (from local file).
    - [ ] Support downloading cover images (metadata fetching).

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

## tasks moved from modules_to_port.md

### src/calibre/devices


#### binatone

- [ ] __init__.py
- [ ] driver.py

#### boeye

- [ ] __init__.py
- [ ] driver.py

#### blackberry

- [ ] __init__.py
- [ ] driver.py

#### cybook

- [ ] __init__.py
- [ ] driver.py
- [ ] t2b.py
- [ ] t4b.py

#### eb600

- [ ] __init__.py
- [ ] driver.py

#### edge

- [ ] __init__.py
- [ ] driver.py

#### eslick

- [ ] __init__.py
- [ ] driver.py

#### hanlin

- [ ] __init__.py
- [ ] driver.py

#### hanvon

- [ ] __init__.py
- [ ] driver.py

#### iliad

- [ ] __init__.py
- [ ] driver.py

#### irexdr

- [ ] __init__.py
- [ ] driver.py

#### iriver

- [ ] __init__.py
- [ ] driver.py

#### jetbook

- [ ] __init__.py
- [ ] driver.py


#### nokia

- [ ] __init__.py
- [ ] driver.py


#### nuut2

- [ ] __init__.py
- [ ] driver.py

#### paladin

- [ ] __init__.py
- [ ] driver.py

#### prs505

- [ ] __init__.py
- [ ] driver.py
- [ ] sony_cache.py

#### prst1

- [ ] __init__.py
- [ ] driver.py


#### sne

- [ ] __init__.py
- [ ] driver.py

#### teclast

- [ ] __init__.py
- [ ] driver.py

### src/calibre/ebooks

#### compression (Legacy Palm/Psion formats - Future Nice-to-Have)

> **Note:** These are obsolete 1990s Palm/Psion compression formats with no public specifications.
> Marked as low priority for future implementation if needed.

- [ ] __init__.py
- [x] palmdoc.c
- [x] palmdoc.py (PalmDoc compression - proprietary Palm format)
- [ ] tcr.py (TCR compression - proprietary Psion format)


##### haodoo

- [ ] __init__.py
- [ ] reader.py

##### palmdoc

- [ ] __init__.py
- [ ] reader.py
- [ ] writer.py
