# calibre-oxide

A Rust port of [calibre](https://calibre-ebook.com/) — ebook library
management, format conversion, a content server, and a desktop app.

> **This is a derivative work of calibre**, which is copyright Kovid
> Goyal and the calibre contributors and licensed GPL v3. calibre-oxide
> is GPL v3 for that reason. See [LICENSE](LICENSE) and
> [NOTICE](NOTICE).

Not affiliated with or endorsed by the calibre project.

## Status

Under active development, and honest about what that means:

| | |
|---|---|
| **Linux** | the development platform; everything is exercised here |
| **Windows** | builds, installs and runs. [CI](.github/workflows/windows.yml) builds and tests on `windows-latest` with MSVC — 5,210 tests |
| **macOS** | never built or tested. Nothing is known to be broken; nothing is known to work |

The desktop app runs on Windows and Linux. What has *not* been verified
by anything automated is the app's runtime behaviour — CI compiles and
tests, it never launches a window.

Platform integration is Linux-only in places and degrades rather than
breaks: device-removal and sleep/resume monitors, "Open With"
application discovery, and system font directories all fall back to
conservative defaults elsewhere. See [docs/WINDOWS.md](docs/WINDOWS.md).

## Building

You need [Rust](https://rustup.rs/) and [Node.js](https://nodejs.org/)
22+. Platform-specific prerequisites — a C++ toolchain, and WebView2 on
Windows — are in [docs/WINDOWS.md](docs/WINDOWS.md).

```
cargo xtask build
```

That is the whole thing. It runs the Rust workspace, then the web UI,
then the desktop app, in that order — which matters, because the app
bundles the other two and building them out of order fails somewhere
that does not mention the order.

```
cargo xtask build      # everything, ending in a runnable app
cargo xtask package    # the above, plus installers — see the note below
cargo xtask test       # the Rust suite and the web suite
cargo xtask help       # the rest
```

`build` stops at a runnable application and needs no network.
`package` is separate on purpose: producing installers is the part that
can fail for reasons unrelated to the code — the Linux AppImage bundler
downloads its tooling from GitHub while it runs, so `package` needs
network access to a specific host that `build` does not.

Binaries land in `target/release/`:

| | |
|---|---|
| `calibre_oxide_app` | the desktop application |
| `calibre_srv` | the content server (the app spawns this) |
| `calibredb` | library management from the command line |
| `ebook_convert` | format conversion |
| `ebook-meta` | metadata inspection and editing |

## Layout

```
crates/
  calibre_db            library database, and the calibredb CLI
  calibre_ebooks        formats, metadata, OEB, polish, TTS — the largest crate
  calibre_conversion    the conversion pipeline and ebook_convert
  calibre_srv           content server (HTTP API, OPDS, reader backend)
  calibre_utils         shared utilities
  calibre_customize     plugin registry
  calibre_plugins_wasm  sandboxed WASM plugin host
  calibre_devices       device drivers
  calibre_scraper_worker  headless webview worker for recipe scraping
  calibre_ai            AI provider integrations
web/                    the Vue frontend, served by calibre_srv
app/src-tauri/          the Tauri desktop shell
e2e/                    end-to-end tests against real built binaries
tools/harness/          development harness
xtask/                  build automation (cargo xtask)
old_src/                calibre's own Python source, vendored — see below
```

### `old_src/` is calibre, kept verbatim

The upstream Python source is checked in, unmodified, as the reference
the port is written and verified against. It is 63 MB and some 4,800
files, and it is calibre's work, not this project's. It is never built
or executed — it is read.

Keeping it in-tree is what makes the porting discipline possible:
modules cite the exact upstream file they came from, and a claim about
upstream behaviour can be checked against the source rather than
recalled. Roughly 276 modules carry a `Port of old_src/...` header
naming their origin.

If you are looking for calibre itself, get it from
[calibre-ebook.com](https://calibre-ebook.com/) — the copy here is a
snapshot for reference, not a distribution channel.

## Contributing

[docs/AGENT_PORTING_GUIDE.md](docs/AGENT_PORTING_GUIDE.md) describes how
ports are written and what "done" means. The short version: a port cites
its upstream source, and narrowing is disclosed rather than quietly
omitted.

```
cargo xtask test
```

## Licence

GPL v3 only. See [LICENSE](LICENSE).

[NOTICE](NOTICE) records attribution and the third-party material that
ships with the project, including one component (RAR/CBR support) whose
licence is not GPL-compatible and which therefore affects redistributing
*built binaries* — building from source for your own use is unaffected.
