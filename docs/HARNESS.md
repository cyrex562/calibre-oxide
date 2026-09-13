# Harness Architecture

The harness is a Rust binary at `tools/harness/`. It orchestrates iterative
porting of the legacy Python calibre codebase (`old_src/`) into the Rust
crates under `crates/`, plus new fault-tolerance and organizational features.

**`run` and `sweep` are deliberately unimplemented (issues #91/#92,
closed as won't-implement, not deferred).** The sections below describe
the bootstrap PR's original autonomous design for them — spawn `claude`
subprocesses to plan/implement/judge each port, then unsupervised
`gh pr merge --squash --auto` on a passing AI-judge verdict — kept here
as a historical record of the design, not a spec to build. In practice
every real port in this repo has landed through an interactive Claude
Code session (`/loop` and similar) doing the same claim/plan/implement/
judge/merge work directly, with a human present at the merge decision
instead of a second, unsupervised AI judge. `scan-placeholders`,
`seed-issues`, `status`, and `playtest-ready` are real and current; the
`run`/`sweep` CLI surface stays wired up (so invoking them fails with a
clear message) but neither has, or will get, a real implementation.

## Invocation

```
harness seed-issues              # populate GitHub issues from modules_to_port.md + placeholders.jsonl
harness scan-placeholders        # rebuild placeholders.jsonl from the code
harness status                   # print in-flight issues, PRs, and last run summary
harness playtest-ready           # emit a per-cluster checklist of what changed since the last playtest tag
harness run ...                  # NOT IMPLEMENTED (deliberately -- see above)
harness sweep                    # NOT IMPLEMENTED (deliberately -- see above)
```

## Iteration loop (historical design, not implemented — see above)

For each issue the harness *would have* worked:

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

## Concurrency (historical design, not implemented)

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

## Judge rubric (historical design, not implemented)

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

## Model routing (historical design, not implemented)

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

## Cross-validation via Python calibre (historical design, not implemented)

The automated `calibre-debug`-discovery flow described below was never
built. Real cross-validation in this repo happens manually, per-issue,
using whatever technique fits (see e.g. the lxml-shaped fake-tree
harness used for several `css_selectors`/`tts` ports) -- not through
this automated pipeline.

Calibre proper doesn't install cleanly from source on Windows (per
`old_src/INSTALL.rst`) — it requires a huge native-dep dev environment.
Instead, the harness invokes the **official Calibre binary**, which
bundles Python + all C extensions and can run arbitrary calibre Python:

- Windows: `C:\Program Files\Calibre2\calibre-debug.exe`
- Linux: `calibre-debug` (from the distro's `calibre` package)
- macOS: `/Applications/calibre.app/Contents/MacOS/calibre-debug`

The harness discovers this at startup and stores the path in
`.harness/config.toml`. If no binary is found, cross-validation is
skipped with a warning and the judge is informed — missing infrastructure
never causes test failures.

Cross-validation pattern per format:

1. Fixture files live in `.harness/fixtures/<subsystem>/`.
2. For each fixture, the harness runs both:
   - `calibre-debug -c "<py snippet that prints canonical JSON>"`
   - The Rust equivalent (e.g. `cargo run -p calibre_ebooks --bin ebook-meta`).
3. Diff outputs. Byte-for-byte where the format is deterministic (JSON
   with sorted keys, sorted OPF, etc.); structural diff otherwise.
4. The judge sees the diff and either accepts (semantic match) or
   rejects with a specific reason.

## Playtest flow

`harness playtest-ready [--since <ref>] [--no-tag]` is a standalone
reporting command over already-merged history — it does not depend on
the autonomous `run`/`sweep` pipeline above (which, in practice, has
never been the way real work lands here; every real port merges via
`gh pr merge --squash` directly, not through the judge loop). When run,
it:

1. Resolves `--since` (explicit ref, else the most recent
   `playtest-<timestamp>` tag, else `origin/master`).
2. Walks `git log --no-merges <since>..HEAD` — every real merge in this
   repo is a squash commit, never a two-parent merge commit, so this is
   the real analogue of "walk merges since `since`".
3. Groups each commit by cluster, inferred from its changed files'
   crate directory (`crates/calibre_db` -> `db`, etc. — the same
   mapping `seed-issues` uses for placeholders).
4. Emits `.harness/playtest/<timestamp>.md`: each commit's subject, its
   `## Summary` bullets if its body has one (real content when the
   commit itself carries a PR description), and the files it touched.
   This deliberately doesn't fabricate "what to click" — the tool has
   no way to know what a change looks like in a running app; a human
   reads the summary and files to judge that.
5. Tags `HEAD` as `playtest-<timestamp>` (unless `--no-tag`), so the
   next invocation's default `--since` picks up from here.

Ingesting playtest feedback into new issues is not implemented — no
real workflow in this repo currently produces that feedback in a form
the harness could parse.

## State

`in_flight`/`green_judged_prs`/`last_sweep` below are only ever written
by the never-implemented `run`/`sweep` pipeline and stay empty in
practice; `last_seed` (written by the real `seed-issues`) is the only
field any current command actually populates.

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
