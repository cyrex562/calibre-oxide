# Rust Porting Guidance for Agents

When porting `old_src` (Python) to `calibre-oxide` (Rust), follow these guidelines to ensure safety, idiomatic code, and maintainability.

## 1. Safety First
- **Avoid `unwrap()`**: Always handle errors with `Result`. Use `?` integration.
  - *Bad*: `file.open()?.read_to_string(&mut content).unwrap();`
  - *Good*: `fs::read_to_string(path).context("Failed to read config")?` (using `anyhow` or `thiserror`).
- **Minimize `unsafe`**: Only use `unsafe` if interfacing with C-libs directly. For Python interop, use `pyo3` safely.

## 2. Python to Rust Mapping
| Python Concept | Rust Equivalent | Notes |
|----------------|-----------------|-------|
| `None` / `Exception` | `Option<T>` / `Result<T, E>` | Explicit nullability & error handling. |
| Inheritance | Traits | Prefer composition + Traits over inheritance. |
| `dict` | `HashMap` or `Struct` | Use strong types (Structs) where keys are known. |
| `list` of mixed types | `Enum` or `Vec<Box<dyn Trait>>` | Enums are preferred (sum types). |
| `contextlib` (with) | RAII / Drop trait | Resources clean themselves up. |

## 3. Project Structure
- **Crates**: Don't dump everything in `src/main.rs`. Break logic into local crates in `crates/`.
- **Modules**: Mirror the Python structure where it makes sense, but simplify deeply nested hierarchies.
  - `src/calibre/db/backend.py` -> `crates/calibre_db/src/backend.rs`

## 4. Dependencies
- **Database**: Use `rusqlite` for SQLite interactions (Calibre uses SQLite extensively).
- **GUI**: Use `iced` for the frontend.
- **Async**: Use `tokio` for I/O bound tasks (device communication, network).
- **Error Handling**: `thiserror` for library crates, `anyhow` for applications.

## 5. Porting Workflow
1. **Isolate**: Pick a single Python module (e.g., `calibre.utils.config`).
2. **Define**: Create the Rust struct/interface.
3. **Implement**: Port the logic.
4. **Test**: Write a Rust unit test. If possible, compare output with the Python version.

## 6. Important Crates for Calibre
- `zip` / `tar`: For archive handling (epub/cbz).
- `xml-rs` / `roxmltree`: For parsing OPF/XML metadata.
- `reqwest`: For network scraping.
- `image`: For cover processing.

## 7. Documentation
- Write `///` doc comments for all public structs and functions.
- Reference the original Python file/function in the docs for traceability.
  - `/// Port of function `foo` in `src/calibre/utils/foo.py``
