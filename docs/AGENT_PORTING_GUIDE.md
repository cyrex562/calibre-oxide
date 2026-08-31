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
- **JS-rendering (real headless browser needs)**: `app/src-tauri` (the actual desktop app) is built on Tauri, so `wry`/`tao` -- the same OS-native webview stack Tauri uses -- are available for ports that genuinely need to execute JavaScript to produce usable content (e.g. `calibre.scraper.webengine_backend`, ported as `crates/calibre_scraper_worker`, a small standalone binary crate spawned over a JSON-lines protocol by `calibre_ebooks::scraper::WebEngineBrowser`). Keep the `wry`/`tao` dependency isolated in its own worker-process crate rather than pulling it into a library crate directly -- it needs a real (or virtual, e.g. `xvfb-run`) display to run at all, which most library crates and headless/CLI contexts shouldn't be forced to require.

## 5. Porting Workflow
1. **Isolate**: Pick a single Python module (e.g., `calibre.utils.config`).
2. **Define**: Create the Rust struct/interface.
3. **Implement**: Port the logic.
4. **Test**: Write a Rust unit test. If possible, compare output with the Python version.

### 5a. Split issues that outgrow one iteration

An "iteration" is one plan→implement→test→PR→merge cycle for a single
coherent chunk of work. If an issue is going to take more than one
iteration to reach a mergeable state, **stop and split it** into
smaller issues rather than letting it grow into a multi-PR mega-issue
tracked by ad hoc checkboxes:

- Split along real seams: one file, one endpoint family, one
  self-contained feature, or one piece of missing infrastructure
  (e.g. "add a `field_metadata` subsystem" is its own issue, separate
  from every feature that will eventually consume it).
- Each new issue should be independently mergeable — completing it
  should not require any of the others to also be done in the same
  PR. It's fine for one issue to be *blocked on* another (say so in
  the issue body); it's not fine for two issues to need the same PR.
- Give each new issue the same definition-of-done as any other port
  issue (real signatures, tests, cross-validation where applicable) —
  splitting is about scope, not about lowering the bar.
- When a large module clearly needs infrastructure this codebase
  doesn't have yet (a job queue, multi-library support, a rendering
  pipeline), don't silently defer it inside a bigger issue's checklist
  — file it as its own issue stating what's missing and what it would
  unblock, even if nobody works it immediately. A visible, honestly-
  scoped backlog issue is more useful than an optimistic checkbox that
  never gets checked.
- If an existing issue has already grown past this (e.g. it's
  accumulated several "Phase N" PRs against one issue number), split
  its *remaining* scope into fresh issues and leave the original as a
  tracking/epic issue linking to them — don't retroactively rewrite
  history for the phases already merged.

## 6. Important Crates for Calibre
- `zip` / `tar`: For archive handling (epub/cbz).
- `xml-rs` / `roxmltree`: For parsing OPF/XML metadata.
- `reqwest`: For network scraping.
- `image`: For cover processing.

## 7. Documentation
- Write `///` doc comments for all public structs and functions.
- Reference the original Python file/function in the docs for traceability.
  - `/// Port of function `foo` in `src/calibre/utils/foo.py``
