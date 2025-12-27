# Porting Plan: Calibre-Oxide

## Goal
Port the legacy Python-based Calibre codebase (`old_src`) to a modern, high-performance Rust application ("Calibre-Oxide") using `iced` (or `egui`) for the GUI.

## Source Analysis
The `old_src` directory contains a massive Python codebase.
Key challenges:
- **Size**: Thousands of files, requiring incremental porting.
- **Dependencies**: Heavy reliance on Qt (`gui2`), which must be replaced by a Rust-native GUI.
- **Complexity**: Deep logic for e-book conversion (`ebooks`), database management (`db`), and device drivers (`devices`).

## Architecture Strategy

### 1. Workspace Structure
We will use a Rust Workspace to modularize the port.
```
calibre-oxide/
├── Cargo.toml (workspace root)
├── src/ (main binary)
├── crates/
│   ├── calibre_db/      # Port of src/calibre/db
│   ├── calibre_ebooks/  # Port of src/calibre/ebooks
│   ├── calibre_devices/ # Port of src/calibre/devices
│   ├── calibre_utils/   # Port of src/calibre/utils
│   └── calibre_gui/     # New Iced/Egui application
```

### 2. GUI Framework Selection
**Recommendation: Iced**
- **Pros**: Native look-and-feel potential, type-safe, Elm architecture fits state management well.
- **Cons**: Still maturing, complex widget creation compared to egui.
- **Why**: Calibre is a consumer desktop app where aesthetics and standard behavior matter. Iced offers a better path to a "polished" tool than Egui (which is excellent but often looks like a developer tool).

### 3. Phased Approach

#### Phase 1: Foundation & Utilities
- **Goal**: Establish the workspace and core types.
- **Tasks**:
    - Set up `Cargo.toml` workspace.
    - Port `src/calibre/constants.py` and basic `src/calibre/utils` to `calibre_utils` crate.
    - Implement logging and configuration loading.

#### Phase 2: The Data Layer (Database)
- **Goal**: Read the existing metadata.db (SQLite).
- **Tasks**:
    - Analyze `src/calibre/db`.
    - Create `calibre_db` crate using `rusqlite`.
    - Implement the schema compatibility to read existing Calibre libraries.
    - **Deliverable**: CLI tool to list books from a library.

#### Phase 3: Core Logic (E-books & Devices)
- **Goal**: Handle file formats and devices.
- **Tasks**:
    - Port `src/calibre/ebooks` - start with metadata reading (opf, mobi, epub).
    - Port `src/calibre/devices` - start with generic USB mass usage.
    - **Deliverable**: CLI tool to read metadata from an epub file.

#### Phase 4: The GUI (The Big Shift)
- **Goal**: Replace `gui2` (Qt).
- **Tasks**:
    - Initialize `iced` application in `calibre_gui`.
    - Implement the Main Window (Library View).
    - Connect `calibre_db` to the GUI for listing books.
    - Implement Book Details panel.
    - **Deliverable**: Minimal functional GUI showing the library.

#### Phase 5: Feature Parity
- **Goal**: Conversion, Editing, Viewer.
- **Tasks**:
    - Implement E-book viewer (web-based or native rendering).
    - Implement Conversion pipeline (this is the hardest part, might assume `Command` calls to old python binary initially, or full rewrite).

## Risk Management
- **Interop**: We may need to keep some Python for obscure format conversions. Use `pyo3` to wrap legacy Python modules if a pure Rust rewrite is too costly in the short term.
- **Testing**: Python has established tests. We should try to run them against the Rust binaries (integration tests) or port them to pure Rust unit tests.
