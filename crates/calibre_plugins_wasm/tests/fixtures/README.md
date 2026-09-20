# WASM test fixtures

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

## Why the binary is checked in

So the host's test suite does not require a WASM toolchain. Its full
source (`probe_plugin/`) and build script (`build.sh`) are checked in
beside it, so it stays reproducible and auditable rather than being an
opaque blob.

## Rebuilding

```sh
rustup target add wasm32-unknown-unknown
./build.sh
```

`probe_plugin/` sets its own empty `[workspace]` deliberately: it
targets `wasm32-unknown-unknown` and must not be built as part of the
host workspace.
