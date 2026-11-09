# Vendored vt100 Parser: the Cursor-Sequence Patch

mllm vendors `vt100 0.18.1` (crates.io) under `vendor/vt100/` with a minimal, audited patch so
child TUI output (OpenCode/OpenTUI and similar) renders row-accurately in panes. This page is the
authoritative reference for why the vendor exists, what the patch changes, how to keep it in sync
with upstream, and its known limitations. Ships in v0.5.1 (commit `d203798`, two commits after the
v0.5.0 tag).

## 1. Why vendored

The `vt100 0.18.1` parser silently ignores four cursor sequences — it has no dispatch arms for
them, so bytes are consumed with no effect on parser state:

| Sequence | Meaning | Upstream 0.18.1 behavior |
|---|---|---|
| `CSI s` | Save cursor (SCOSC) | silently ignored |
| `CSI u` | Restore cursor (SCORC) | silently ignored |
| `ESC D` | Index / IND (down one row, scroll at bottom margin) | silently ignored |
| `ESC E` | Next line / NEL (CR + LF) | silently ignored |

Full-screen TUIs rely on these when redrawing. OpenTUI (the renderer behind OpenCode v2.2.6) uses
`CSI s`/`CSI u` around redraws and `ESC D` for line motion; `ESC E` (NEL) is handled for
completeness. Inside an mllm pane the parser's cursor therefore diverges from where a real
terminal's cursor would be; OpenTUI's next draws land one row too high and overwrite or erase
existing lines. Observed symptom: *"lines move back to the
above line and overwrite them"* in every `opencode` pane.

The fix is vendoring with a minimal patch rather than adopting a community fork: the delta stays
small, auditable, and ours (fork policy in §3, limitations in §7). The override is wired in
`Cargo.toml`:

```toml
[patch.crates-io]
vt100 = { path = "vendor/vt100" }
```

## 2. The patch

The entire source delta vs pristine upstream 0.18.1 is four dispatch arms in
`vendor/vt100/src/perform.rs`; every other file (`Cargo.toml`, `LICENSE`, `CHANGELOG.md`, …) is
byte-identical to the crates.io package and the version stays `0.18.1`:

| Arm | Maps to | Notes |
|---|---|---|
| `ESC D` | `lf()` (IND) | honors the scroll region and fires `on_scroll`, like a real terminal's linefeed |
| `ESC E` | `cr()` + `lf()` (NEL) | CR + LF |
| `CSI s` | `decsc()` (save cursor) | reuses upstream's DECSC, already reachable via `ESC 7` |
| `CSI u` | `decrc()` (restore cursor) | reuses upstream's DECRC, already reachable via `ESC 8` |

Why `CSI s` maps unconditionally to save-cursor: upstream vt100 implements no DECLRMM (`?69`) or
left/right-margin state at all, so the parser can never be in DECSLRM mode and the SCOSC reading is
the only one a client can meaningfully trigger. No mllm client sets `?69`. The full caveat is in
§7 (known limitations).

## 3. Syncing with upstream

The pristine upstream source we vendored is in the local cargo registry cache — byte-identical to
crates.io 0.18.1, no network checkout needed. To audit the current delta or re-apply the patch
after pulling a newer upstream version:

```bash
# Audit the delta (expected: only `src/perform.rs` differs; the diff also lists
# four crate-metadata files that were not vendored)
diff -ru ~/.cargo/registry/src/index.crates.io-*/vt100-0.18.1 vendor/vt100
```

1. Re-apply the four arms from §2 to `vendor/vt100/src/perform.rs` (keep the mllm pane-bug
   comments).
2. Re-run the regression tests: `cargo test test_vt100_patch` (§4) — all three must pass.
3. Confirm the override is active: `cargo tree -i vt100` must resolve via the `vendor/vt100` path.

**Fork policy**: a community fork exists (`vt100-psmux`, repo `psmux/vt100-rust-patched`) with the
same `CSI s`/`u` save-restore patch plus rendering-fidelity extras, but it has **no IND/NEL
(`ESC D`/`E`) arms** — adopting it would require re-adding them. mllm stays on its own vendored
0.18.1 + minimal patch. If you consult the fork for extra fixes, read the **published
`vt100-psmux 0.16.13` crate** — the fork's GitHub `main` branch is stale and effectively unpatched
there. The fork is based on vt100 0.16.x and adds `Attrs` fields, so its extras are not drop-in
for our vendor (§7).

## 4. Test coverage

Three regression tests in `src/pty.rs::tests` lock the patch in (each fails without it):

| Test | Locks |
|---|---|
| `test_vt100_patch_cursor_save_restore` | `CSI s` saves the cursor; `CSI u` restores it and subsequent draws land at the restored position |
| `test_vt100_patch_ind_nel` | `ESC D` moves down preserving the column; `ESC E` moves down to column 0 |
| `test_vt100_patch_ind_scrolls_at_bottom` | `ESC D` on the bottom row scrolls (top row pushed into scrollback, visible rows shift, cursor pinned to the bottom row with its column preserved) |

All three are unit tests, counted in the unit bucket. Suite total at the time of writing:
**100/100 green** (64 unit + 32 integration + 4 audit regression; the regression bucket lives in
`tests/reviewer_audit_regressions.rs` and covers the Protocol v1.3 audit findings, not this patch).

## 5. Pane-fidelity verification

Rendering fidelity of the full pane pipeline — raw child bytes → vt100 parser → `PtyTerminalWidget`
→ ratatui diff → host-terminal bytes — is re-verifiable end-to-end with an opt-in replay harness.
Both tests are no-ops without their environment variables, so CI is unaffected.

**Replay tests** (`src/ui.rs::tests`):

```bash
# Single capture through the real widget path
REPLAY_CAPTURE=<raw capture> [REPLAY_COLS=60] [REPLAY_ROWS=30] \
  cargo test replay -- --nocapture

# Multi-frame host emission: splits the capture at synchronized-frame markers
# (ESC[?2026l), runs parser + widget per frame with ratatui-diff emulation, and
# writes the resulting host-byte stream for comparison
REPLAY_MULTIFRAME=<raw capture> [REPLAY_OUT=/tmp/opencode/mllm_host.bin] \
  [REPLAY_COLS=117] [REPLAY_ROWS=49] \
  cargo test multiframe -- --nocapture
```

**Ground truth**: render the same byte stream with the wide-char-aware reference emulator and
diff the screens:

```bash
python3 scripts/ref_emu.py <byte stream file> <rows> <cols>
```

`scripts/ref_emu.py` models the host terminal for the sequences OpenCode/ratatui emit (CUP, CUU/
CUD/CUF/CUB, EL, ED, CR, LF, BS, IND, RI, DECSC/DECRC, `CSI s`/`u`, SGR ignored, private modes
ignored) with wide chars occupying two columns — the critical behavior for pane fidelity.

**Capture tooling** (stdlib-only; geometry/behavior via `CAPTURE_*` env vars, `CAPTURE_OUT`
required):

| Script | Purpose |
|---|---|
| `scripts/capture_raw.py` | raw child-stream PTY capture |
| `scripts/capture_bidir.py` | bidirectional capture that emulates mllm's query responder (`CSI 6n`, `OSC 10/11`, XTGETTCAP) |
| `scripts/capture_session.py` | full-session capture with a rich prompt (emoji/markdown) for realistic frames |

**Provenance**: this harness reproduced the row-overwrite symptom byte-for-byte (pre-fix host
emission rendered one row high vs ground truth) and, with the patch applied, **the validating run
was 83 synchronized frames (117×49), all identical to ground truth** — that frame count is
*not* a reproducible invariant (no capture fixture is tracked in-repo). Reproduction of the
verification is the loop itself: capture a real session → replay → compare against
`scripts/ref_emu.py` ground truth.

## 6. README pointer

The README references this page with a short pointer under *Architecture Overview* (§1.1) — the
single location (no duplicate in the build section). Canonical text, kept in sync with this
document:

> **Vendored vt100 parser**: mllm vendors `vt100 0.18.1` under `vendor/vt100/` with a minimal
> four-arm patch (cursor save/restore `CSI s`/`u`, IND/NEL `ESC D`/`E`) that upstream lacks and
> OpenTUI-driven TUIs require — without it, panes redraw on wrong rows. See
> [docs/vendored-vt100.md](docs/vendored-vt100.md) for rationale, upstream sync, the pane-fidelity
> replay harness, and known limitations.

## 7. Known limitations (consolidated)

One authoritative list; §2 and §3 point here instead of duplicating these caveats.

**7.1 — `CSI s` is SCOSC only (no DECLRMM support upstream).** The patch maps bare `CSI s` to
save-cursor unconditionally. Upstream vt100 0.18.1 implements no `?69` (DECLRMM) or left/right
margin state, so DECSLRM mode is unreachable and no ambiguity is introduced. If a future client
sets `?69` and expects DECSLRM behavior, that is a pre-existing upstream vt100 limitation to
evaluate then — nothing the patch introduced.

**7.2 — SGR 2, 5, 8, 9 are not rendered.** Parser and widget coverage, verified against
`vendor/vt100/src/` and `src/ui.rs`:

- **SGR 5 (blink) / 9 (strikethrough)**: not tracked by the parser's `Attrs` — dropped; affected
  text renders plain.
- **SGR 8 (conceal)**: not tracked, so text meant to be hidden renders **visibly** in panes.
  *Security note*: concealing via SGR 8 does not protect content in mllm panes. Even if the paint
  were suppressed, hidden text would still leak through the other text paths — **selection and
  clipboard today, and any future search** (mouse selection / `extract_selected_text`, OSC 52
  clipboard export; no pane search exists in mllm at the time of writing). A complete fix would need
  parser + `Attrs` + widget changes *and* suppression on every leak path, not just painting.
  No current client emits SGR 8 (flagged by Researcher; consciously documented, not patched).
- **SGR 2 (dim)**: parsed and tracked (`cell.dim()` in `vendor/vt100/src/cell.rs`) but the widget
  never applies `Modifier::DIM` (`src/ui.rs` maps only bold / italic / underline / inverse), so
  dim text renders at normal intensity.

*No driver today*: a proper SGR state-machine pass over all five captured child-stream sessions
(`oc_bidir`, `oc_real`, `oc_rich`, `oc_rich_noenv`, `oc_w` — investigation captures, not shipped;
`mllm_host.bin` is rendered host output, not a client stream) found only SGR `0/1/3/49` —
zero occurrences of 5, 8, or 9. (A naive scan "finds" 5/8/9 inside extended-color arguments such
as `38;5;n` and `48;2;r;g;b`; those are color parameters, not SGR 5/8/9.) SGR 2 and 39 also never
appear. These are documentation-only gaps until a capture shows a driver.

**7.3 — Fork citation policy.** When auditing or borrowing from the community fork, read the
**published `vt100-psmux 0.16.13` crate** (crates.io, all releases 0.16.x), not the GitHub
`psmux/vt100-rust-patched` `main` branch — `main` is stale and effectively unpatched upstream
there. The fork is two minors behind our 0.18.1 vendor and extends `Attrs` with extra fields, so
its rendering-fidelity extras (SGR 5/8/9, extended underline styles/colors, OSC 8 hyperlinks) are
not drop-in; adopting any of them would be a parser + `Attrs` + widget change (see also §3).
