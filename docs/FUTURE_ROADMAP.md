# Future Roadmap

**This file's previous contents were stale and have been replaced (2026-09-20).**

It described conversion as "Not Started" and planned a GUI built on Iced. Both
were wrong by a wide margin: the conversion pipeline is real and complete
(`Plumber::run()`, 20 input and 15 output plugins, a real `ebook-convert`
binary), and the Iced prototype was abandoned in favour of a Tauri shell around
the `web/` browser UI. Anyone planning work from the old text would have been
misled on nearly every line, so it was removed rather than left to rot.

## Where the real roadmap lives

- **[#816](https://github.com/cyrex562/calibre-oxide/issues/816)** — the desktop
  UI parity epic. Phases, priorities and the ready-to-test milestone.
- **Deferred work**, filed and labelled `low-priority`:
  [#811](https://github.com/cyrex562/calibre-oxide/issues/811) devices ·
  [#812](https://github.com/cyrex562/calibre-oxide/issues/812) dormant format
  plugins · [#813](https://github.com/cyrex562/calibre-oxide/issues/813) console
  tools · [#814](https://github.com/cyrex562/calibre-oxide/issues/814) news
  recipes · [#815](https://github.com/cyrex562/calibre-oxide/issues/815)
  storefronts.
- **[`docs/modules_to_port.md`](modules_to_port.md)** — the per-file porting
  ledger, which *is* kept current.

## The one thing worth carrying forward

A recurring finding across this port: a feature is frequently "missing" only in
the sense that nothing calls it. The engine is merged and tested, and no route
or UI reaches it — `oeb::polish` (92 files), `calibre_ai` (six providers), the
TTS engine behind `/tts/synthesize`, HTMLZ/PML conversion plugins, `spellbook`
spell checking.

Before filing something as unported, check whether it is merely unwired. The
issue tracker has been wrong about this repeatedly, and so was this file.
