# Stubs to Complete

The following modules have been ported as stubs or placeholders and need full implementation to match the original Python functionality.

## Devices

| Rust File | Original Python File | Description |
|-----------|----------------------|-------------|
| `crates/lexmechanic_devices/src/android/driver.rs` | `calibre/devices/android/driver.py` | Implementation of Android MTP/ADB communication. |
| `crates/lexmechanic_devices/src/nook/driver.rs` | `calibre/devices/nook/driver.py` | Driver for Barnes & Noble Nook devices. |
| `crates/lexmechanic_devices/src/smart_device_app/driver.rs` | `calibre/devices/smart_device_app/driver.py` | Support for Calibre Companion and similar apps. |
| `crates/lexmechanic_devices/src/kobo/driver.rs` | `calibre/devices/kobo/driver.py` | Driver for Kobo eReaders. |
| `crates/lexmechanic_devices/src/kobo/bookmark.rs` | `calibre/devices/kobo/bookmark.py` | Kobo bookmark handling. |
| `crates/lexmechanic_devices/src/kobo/books.rs` | `calibre/devices/kobo/books.py` | Kobo book metadata handling. |
| `crates/lexmechanic_devices/src/kobo/db.rs` | `calibre/devices/kobo/db.py` | Kobo internal SQLite DB interaction. |
| `crates/lexmechanic_devices/src/kobo/kobotouch_config.rs` | `calibre/devices/kobo/kobotouch_config.py` | KoboTouch specific configuration. |



## Ebook Input Plugins

| Rust File | Original Python File | Description |
|-----------|----------------------|-------------|
| `crates/lexmechanic_ebooks/src/input/chm_input.rs` | `calibre/ebooks/chm/reader.py` / `input.py` | Parsing of Compiled HTML Help (CHM) files. Currently returns a static "Not Supported" page. |
| `crates/lexmechanic_ebooks/src/input/djvu_input.rs` | `calibre/ebooks/djvu/...` | Extraction of text and images from DJVU files. |
| `crates/lexmechanic_ebooks/src/input/recipe_input.rs` | `calibre/web/feeds/recipes/...` | Execution of recipe scripts to fetch news. |
| `crates/lexmechanic_ebooks/src/input/rar_input.rs` | `calibre/utils/unrar.py` | Handling of RAR archives for ebook input (requires unrar lib). |

## Ebook Output Plugins

| Rust File | Original Python File | Description |
|-----------|----------------------|-------------|
| `crates/lexmechanic_ebooks/src/output/docx_output.rs` | `calibre/ebooks/docx/writer.py` | Writing valid DOCX files with full formatting. Currently creates a minimal valid zip structure. |
| `crates/lexmechanic_ebooks/src/output/lrf_output.rs` | `calibre/ebooks/lrf/writer.py` | Writing Sony LRF format. |

## Customize & Plugins

| Rust File | Original Python File | Description |
|-----------|----------------------|-------------|
| `crates/lexmechanic_customize/src/builtins.rs` | `calibre/customize/builtins.py` | Complete registry and discovery of builtin plugins. |
| `crates/lexmechanic_customize/src/zipplugin.rs` | `calibre/customize/zipplugin.py` | Loading plugins from ZIP files. |
| `crates/lexmechanic_customize/src/ui.rs` | `calibre/customize/ui.py` | Interface for GUI actions (likely needs heavy adaptation for Rust/Tauri/Egui). |
| `crates/lexmechanic_customize/src/conversion.rs` | `calibre/customize/conversion.py` | `convert` methods in traits return `Err("Not implemented")`. |

## Database & Utilities

| Rust File | Original Python File | Description |
|-----------|----------------------|-------------|
| `crates/lexmechanic_ebooks/src/conversion/archives.rs` | `calibre/ebooks/conversion/archives.py` | RAR and 7z extraction (currently stubs in `ArchiveHandler`). |
| `crates/lexmechanic_ebooks/src/conversion/preprocess.rs` | `calibre/ebooks/conversion/preprocess.py` | HTML/CSS preprocessing logic. |

## Mobi

| Rust File | Original Python File | Description |
|-----------|----------------------|-------------|
| `crates/lexmechanic_ebooks/src/mobi/tweak.rs` | `calibre/ebooks/mobi/tweak.py` | Explode/Rebuild MOBI functionality. Stubbed due to missing dependencies (`MobiReader`, `Plumber`). |
| `crates/lexmechanic_ebooks/src/mobi/mobiml.rs` | `calibre/ebooks/mobi/mobiml.py` | XHTML to MobiML conversion. Stubbed due to complexity. |
| `crates/lexmechanic_ebooks/src/mobi/utils.rs` | `calibre/ebooks/mobi/utils.py` | `mobify_image` and `rescale_image` are stubs requiring image processing library. |
| `crates/lexmechanic_ebooks/src/mobi/mobi6.rs` | `calibre/ebooks/mobi/reader/mobi6.py` | Implementation of MOBI 6 reading logic (Headers ported, content extraction stubbed). |

