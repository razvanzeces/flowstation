# Bost FlowStation — working notes

Rust TETRA base station (fork of FlowStation / tetra-bluestation) for Raspberry Pi. Workspace:
`crates/tetra-core`, `tetra-config`, `tetra-pdus`, `tetra-saps`, `tetra-entities` (the stack,
dashboard, network entities), `bins/bluestation-bs` (the service binary).

## Where work happens

- **Branch `bost`** is where all work is committed and pushed. Since v0.5.3 upstream made `main`
  the stable OTA branch that installs update from; `bost` stays the working branch. `beta` is not
  used for this project.
- Current release: **v0.5.6** (`BOST_VERSION` in `crates/tetra-core/src/lib.rs`, heading in
  `CHANGELOG.md`). A release bumps both; `## vX.Y.Z — title` headings.
- Two changelogs with the same entries: `CHANGELOG.md` in Spanish (the dashboard OTA reads it from GitHub, keep its format) and `CHANGELOG_en.md` in English. Every change goes into both in the same commit.
- Project status and next steps: [`Docs/multi-cell-status.md`](Docs/multi-cell-status.md).
  Design and per-phase "as built" notes: [`Docs/multi-cell-plan.md`](Docs/multi-cell-plan.md).

## Build and test

```bash
cargo build --workspace
cargo test --workspace --no-fail-fast
cargo check -p bluestation-bs --features asterisk   # Asterisk code only compiles with this feature
```

- Keep builds warning-free; all tests pass as of v0.5.0 (281 `tetra-entities` library tests plus the
  integration suites in `crates/tetra-entities/tests`).
- **Do not run `cargo fmt --all`**: the repo is not rustfmt-clean and it rewrites ~45 unrelated
  files. Format only what you touch, by hand, matching the surrounding style.
- `Cargo.lock` is committed and in sync; a build should not modify it.
- New Brew extension: add it to `HANDOVER_EXTENSIONS` in `crates/tetra-entities/src/net_dashboard/roaming.rs` and give it a category in `roamRenderBrew` (`html.rs`, Infrastructure page, formerly Roaming / Cells, "Server features"), with the label in `LANGS.en` and `LANGS.es`.
- Dashboard JS lives inside `crates/tetra-entities/src/net_dashboard/html.rs` (a `r#"…"#` raw
  string — never write `"#` inside it). Syntax-check by extracting the `<script>` blocks and running
  `node --check`. UI strings go in `LANGS.en` and `LANGS.es` (other languages fall back to EN).

## Conventions

- Commit messages: imperative summary line, short body. No Claude/Anthropic attribution anywhere
  (no Co-Authored-By trailer in commits or merges, nothing in PRs, tags or docs).
- Commits so far used the one-off identity `ysam <ysamouhos@gmail.com>`
  (`git -c user.name=… -c user.email=… commit`) because the first workstation had no git identity.
- Prefer small, reviewed commits; ask before pushing.

## Multi-cell architecture in one paragraph

Each `[[cells]]` entry runs its own radio stack (PHY→CMCE) on its own SDR thread with a config from
`StackConfig::for_extra_cell` (`cell_id`, `site_linked`). With Brew or LST enabled the cells are
*site-linked*: every cell reports registrations/floors/calls/SDS to its `TetraEntity::Brew` slot,
which is `net_site::SiteSwitch` (wrapping the real Brew/LST entity) on the primary and a `CellLink`
on the others. The switch routes by carrier (unique per cell), session UUID and radio location
(`SiteDirectory`), fans calls out, copies voice through per-cell playout buffers, arbitrates one
talker per group, and re-applies the Brew routing rules before the real network sees anything.
`SiteRelay` does the same for Asterisk. MLE handles announced handover (U-PREPARE / D-NEW-CELL,
forward registration, U-RESTORE) with the switch preparing the target cell.
