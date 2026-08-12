# Harness Architecture

The harness is a Rust binary at `tools/harness/`. It orchestrates iterative
porting of the legacy Python calibre codebase (`old_src/`) into the Rust
crates under `crates/`, plus new fault-tolerance and organizational features.

## Invocation

```
harness seed-issues              # populate GitHub issues from modules_to_port.md + placeholders.jsonl
harness scan-placeholders        # rebuild placeholders.jsonl from the code
harness run --issues 12,15,18    # tackle a specific set of issues
harness run --cluster db-cli     # tackle everything labeled `cluster:db-cli`
harness run --auto --max-issues 5  # pick the top-N unblocked issues
harness sweep                    # merge any PRs the harness already marked green-and-judged
harness status                   # print in-flight issues, PRs, and last run summary
```

## Iteration loop

For each issue the harness works:

1. **Claim** — assign the issue to the harness bot, label `in-progress`.
2. **Branch** — `git checkout -b port/<issue-number>-<slug>` off `master`.
3. **Plan** — spawn a Sonnet-tier planning agent with the issue body, the
   Python source file(s) it references, and the fault-tolerance contract.
   Output: a short markdown plan committed to `.harness/plans/<issue>.md`.
4. **Implement** — spawn an implementer agent (Haiku for small, Sonnet for
   medium, Opus for complex — heuristic on Python LOC + import fanout).
   The implementer is instructed to prefer placeholder signatures over
   stubs (see §Placeholder discipline). Every new placeholder is appended
   to `docs/placeholders.jsonl` in the same commit.
5. **Verify locally** — run:
   - `cargo fmt --all -- --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --workspace --all-targets`
   - Cross-validation tests (if a Python calibre venv is available at
     `.harness/py/venv`), diffing the Rust output against the Python
     output for the same input fixtures.
6. **Judge** — spawn a distinct judge agent (Sonnet, no memory of the
   implementer's chain-of-thought). It is given: the diff, the
   fault-tolerance contract, the issue body, the test output, and the
   `docs/AGENT_PORTING_GUIDE.md` rules. It returns `{verdict:
   pass|fail|revise, reasons: [...]}`. Verdict rubric in §Judge rubric.
7. **PR + merge** — on pass: `gh pr create`, wait for CI (none, but the
   harness re-runs tests once against `origin/master` merged into the
   branch to catch drift), then `gh pr merge --squash --auto`. On revise:
   send the judge's reasons back to the implementer for one revision
   pass; if the second attempt also fails, mark the PR `judge-review`
   and stop.
8. **Update state** — `docs/modules_to_port.md`, `docs/placeholders.jsonl`,
   `.harness/state.json`.

## Concurrency

`--max-concurrent N` (default 3) governs how many issues run in parallel.
Each in-flight issue owns its own git worktree under `.harness/worktrees/`,
so builds don't fight. Merges are serialized: only one branch merges at a
time, and every merge triggers a rebase of the other in-flight branches.

## Placeholder discipline

The user-agreed anti-stub rule:

- Do **not** commit a function body that returns a fake value and calls it
  done. Instead, define the real signature (types, docs, errors), and put
  the body as:

  ```rust
  #[calibre_oxide_macros::placeholder(reason = "<why>", python_ref = "old_src/…")]
  fn foo(&self, x: Bar) -> Result<Baz> {
      todo!("placeholder: <one-line description>")
  }
  ```

- The `#[placeholder]` attribute is a no-op at codegen but is grep-friendly.
  The harness's `scan-placeholders` walks the AST and rewrites
  `docs/placeholders.jsonl`.
- Placeholder registry entry schema:

  ```jsonl
  {"crate":"calibre_ebooks","path":"src/mobi/mobiml.rs","symbol":"MobiMlizer::process","python_ref":"old_src/src/calibre/ebooks/mobi/mobiml.py","reason":"XHTML→MobiML transform pending","created":"2026-08-11T23:12:00Z","priority":"medium"}
  ```

- Every iteration, the harness prioritizes clearing existing placeholders
  before opening ports to new files. Concretely, the auto-pick order is:
  1. Bug fix issues.
  2. Placeholder-clearing issues (labeled `placeholder`).
  3. Fault-tolerance issues (labeled `fault-tolerance`).
  4. Port issues in dependency-topological order.

## Judge rubric

The judge answers each of these with yes/no and one line of reasoning.
Verdict is `pass` only if all mandatory items are yes.

**Mandatory**:
- Does every I/O against a library path go through `LibraryHandle`?
- Are there any new `.unwrap()` or `.expect()` in production code?
  (Tests OK.)
- Are all `todo!()` bodies wrapped in `#[placeholder]` and registered?
- Do all new tests actually exercise the code they claim to (i.e., not
  tautological)?
- Do the cross-validation tests pass if run?
- Does the PR touch anything outside the issue's declared scope?
  (Scope creep = fail.)

**Advisory** (does not block pass, but generates a follow-up issue):
- Are error types specific enough (not `anyhow::Error` in library crates)?
- Are public APIs documented with `///`?
- Is the code idiomatic Rust vs a mechanical Python transliteration?

## Model routing

| Role                | Default model    | Escalate to Opus when                       |
| ------------------- | ---------------- | ------------------------------------------- |
| Planner             | Sonnet 4.6       | Python module >800 LOC or 5+ external deps  |
| Implementer         | Haiku 4.5        | Planner requested it, or 2nd revision       |
| Judge               | Sonnet 4.6       | Never — keep the judge cheap and consistent |
| Cross-val fixture writer | Haiku 4.5   | —                                            |
| Placeholder-clearer | Sonnet 4.6       | Placeholder priority=high                    |

The harness enforces model routing by passing `--model` to `claude` calls.
Every call is logged to `.harness/logs/calls.jsonl` with token counts so we
can true-up cost weekly.

## Cross-validation via Python calibre

If `.harness/py/venv/bin/python` (or `Scripts/python.exe` on Windows) can
import `calibre`, the harness will:

1. Discover matching Python entry points for each new Rust port
   (e.g., `ebook-meta` in the metadata subsystem).
2. Run the Python command against fixtures in `.harness/fixtures/<subsystem>/`.
3. Run the Rust command against the same fixtures.
4. Diff outputs. Byte-for-byte where the format is deterministic (JSON
   with sorted keys, sorted OPF, etc.); structural diff otherwise.

If the venv is missing or Python calibre fails to import, cross-validation
is skipped with a warning and the judge is told so — no failing tests for
missing infrastructure.

## Playtest flow

When the user runs `harness playtest-ready`, the harness:

1. Rebases and re-verifies each green-judged branch against latest master.
2. Merges them.
3. Emits a per-cluster checklist to `.harness/playtest/<timestamp>.md`
   with: what was added, what to click, what to look for. This is the
   "list of things to check" the user asked for.
4. On feedback prompt from user, the harness ingests the feedback, opens
   one issue per distinct concern, and prioritizes them ahead of new port
   work.

## State

`.harness/state.json` — small, human-readable, atomically written:

```json
{
  "in_flight": [
    {"issue": 42, "branch": "port/42-metadata-opf", "started": "...", "worktree": ".harness/worktrees/wt-42"}
  ],
  "green_judged_prs": [17, 21],
  "last_sweep": "2026-08-11T20:00:00Z"
}
```

No secret material lives here. `gh` auth is in the OS keyring, model
credentials are in env vars.
