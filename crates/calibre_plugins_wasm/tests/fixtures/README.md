# WASM test fixtures

Two real WebAssembly plugins, both with source and build script checked in.

`probe_plugin.wasm` is a **real** WebAssembly plugin used by
`../real_wasm_plugin.rs` to prove the host in `calibre_plugins_wasm`
actually loads, sandboxes and runs third-party code (issue #798).

It exports one function per property under test:

| export      | proves                                              |
|-------------|-----------------------------------------------------|
| `echo`      | bytes really cross into the sandbox and back         |
| `uppercase` | the plugin really computes, rather than echoing      |
| `boom`      | a trapping plugin is contained; the host survives    |
| `spin`      | an infinite loop is killed by the declared timeout   |
| `fetch`     | network is denied when no `allowed_hosts` declared   |

## `banner_plugin.wasm`

A realistic **third-party file-type plugin** (issue #799), used by
`../file_type_abi.rs`. It implements only the documented
`run_file_type` export, declares no capabilities at all, and stamps a
banner onto imported `.txt` content -- deliberately shaped like a
plugin an outside author would actually write.

## Why the binaries are checked in

So the host's test suite does not require a WASM toolchain. Their full
sources and the build script (`build.sh`) are checked in beside them, so
they stay reproducible and auditable rather than being opaque blobs.

## Rebuilding

```sh
rustup target add wasm32-unknown-unknown
./build.sh
```

Each fixture crate sets its own empty `[workspace]` deliberately: they
target `wasm32-unknown-unknown` and must not be built as part of the
host workspace.
