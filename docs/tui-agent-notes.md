# TUI Agent Notes: Terminal Queries, Wide Glyphs & `OPENTUI_*` Fallbacks

Operational notes for running full-screen TUI agents (OpenCode/OpenTUI and similar) inside mllm panes.
Reference implementation: `src/pty.rs::build_query_responses()` (PTY reader thread) and the wide-glyph
handling in the `PtyTerminalWidget` (`src/ui.rs`). Applies to mllm ≥ v0.5.0 with the OpenTUI
pane-rendering fix.

## 1. Problem

Full-screen TUIs probe the host terminal at startup. A multiplexer that never answers these probes
causes two failure modes:

1. **Startup stalls / degraded rendering** — clients block or fall back blindly while waiting for
   query responses (e.g. cursor-position and color queries).
2. **Wide-glyph truncation** — clients whose cell-width math disagrees with the multiplexer's
   grid parser paint wide glyphs (emoji, icons) over their own continuation cell, clipping the
   right half of the glyph.

mllm addresses mode 1 by answering the standard probes (§2) and mode 2 by (a) skipping
`is_wide_continuation()` cells in the widget so ratatui's diff-based redraw keeps the right half
of each glyph, and (b) recommending `OPENTUI_FORCE_WCWIDTH=1` so the client's width math aligns
with the `vt100` parser (§4).

## 2. OpenTUI Startup-Query / Response Matrix

Queries are scanned per raw PTY read chunk (`build_query_responses`, in the reader thread — not
on the UI tick path).

| Probe | Meaning | mllm response |
|---|---|---|
| `CSI 6n` | Device Status Report (cursor position) | **Answered**: `CSI <row>;<col>R`, 1-based, from the parser's cursor state *after the whole chunk is processed* (see Known limitations) |
| `OSC 10;?` | Foreground color query | **Answered**: `OSC 10;rgb:c7c7/c7c7/c7c7 ST` (light gray) |
| `OSC 11;?` | Background color query | **Answered**: `OSC 11;rgb:0000/0000/0000 ST` (black — matches the assumed dark host theme) |
| `DCS +q Pt;Pt` | XTGETTCAP terminal-capability query | **Answered**: `DCS +r ST` (empty = capability not supported; client falls back safely) |
| `CSI ? u` | Kitty keyboard-protocol query | **Silence** (deliberate) |
| `OSC 1337` | iTerm2 capabilities/images | **Silence** (deliberate) |
| `OSC 66` | Kitty Text-Sizing Protocol (width hints / scaled text) | **Silence** on the draw; CPR detection resolves to "unsupported" (§3) |
| `OSC 7501` | Program Status Protocol (Ghostty) | **Silence** on the `?` query; subsequent status pushes are harmlessly ignored (§3) |
| Kitty graphics / sixel, `OSC 9/99/777` notifications | Optional graphics / notifications | **Silence** (no reply path; clients fall back) |

**Why silence is the correct decline**: optional protocols have no negative-ack; clients treat a
timed-out or unanswered probe as "unsupported" and use legacy fallbacks. Answering with malformed
or wrong-shape data would be worse than silence.

**Known limitations**: (1) a query split across two PTY read chunks is missed. (2) `CSI 6n` replies
are computed from the parser's cursor state **after the entire chunk is processed**, and only one
reply is pushed per query type per chunk (occurrences are counted but not honored), so a position
is exact only when the client issues each CPR in its own write and waits for the reply before
drawing further — which OpenCode/OpenTUI v2.2.6 does (verified by bidirectional capture). A client
that batches several CPRs into a single write would get too few replies, with positions sampled at
end-of-chunk. Neither is observed in practice.

## 3. Protocol Notes (verified against OpenTUI/OpenCode v2.2.6 behavior)

### 3.1 OSC 66 — CPR-trick detection, not a query/response

OpenTUI does not detect OSC 66 support via a response frame. It emits:

```
CR + CSI 6n + OSC 66;w=2;␠ + CSI 6n + OSC 66;s=2;␠ + CSI 6n
```

then compares the three cursor positions:

- All identical → width & scale unsupported.
- +2 columns after the `w=2` draw → explicit width supported.
- +2 more after the `s=2` draw → scaled text supported.

Because the `vt100` parser ignores OSC 66 draw sequences, the cursor never advances, mllm's three
`CSI 6n` answers are identical, and OpenTUI correctly concludes "no OSC 66" and falls back to
single-width rendering. No stall, no mllm-side action needed.

### 3.2 OSC 7501 — Program Status Protocol

A program→terminal status *push* (idle/working/blocked/done/error), designed by Ghostty. OpenTUI
sends an `OSC 7501;?` detection query at startup, but `setProgramStatus()` does **not** wait for
detection — OpenCode emits status reports unconditionally, and terminals without support ignore
them. Silence ⇒ `capabilities.program_status = false`; reports are harmlessly ignored.

## 4. `OPENTUI_*` Fallback Environment Set

OpenCode v2 delegates rendering to OpenTUI. OpenCode's own `tui.json` exposes only
theme/diff_style/cursor/mouse/attention — there is no renderer-protocol knob and no dedicated
low-fancy CLI flag. The supported levers are environment variables OpenCode passes through to
OpenTUI:

| Variable | Effect | Notes |
|---|---|---|
| `OPENTUI_FORCE_WCWIDTH=1` | Force wcwidth cell-width math | Aligns the client with mllm's `vt100` parser; directly de-risks the wide-glyph mismatch. **Presence flag** — any set value (even `0`/`false`) enables it; only *unsetting* disables. |
| `OPENTUI_FORCE_EXPLICIT_WIDTH=0` | Skip the OSC 66 width query outright | Belt-and-suspenders; not required, since silence already reads as "unsupported". |
| `OPENTUI_GRAPHICS=0` | Disable kitty/sixel graphics probes | Silence already declines these safely. |
| `OPENTUI_IMAGE_PROTOCOL=blocks` | Render images as block characters | Instead of kitty/sixel. |
| `OTUI_USE_ALTERNATE_SCREEN=0` | Force main-screen (inline) rendering | Closest thing to an "alt rendering mode"; changes rendering mode and scrollback behavior. |
| `OPENTUI_NOTIFICATIONS=0` | Disable OSC 9/99/777 notification probes | Silence already declines these safely. |

### 4.1 Adopted configuration (mllm decision)

Only `OPENTUI_FORCE_WCWIDTH=1` is adopted for now — zero behavioral risk, directly fixes the
width-math mismatch. The rest of the set is deferred: `OTUI_USE_ALTERNATE_SCREEN=0` changes
rendering/UX, and the graphics/notification variables address probes that silence already
handles.

Adoption is **config-level, not hardcoded in the PTY spawner**: the spawner is generic across
agy/claude/opencode clients, and baking OpenTUI-specific variables into it would couple the
transport layer to one TUI framework and pollute every client's environment. Prefix the client
command in `op.yaml` / `mllm.yaml` instead:

```yaml
command: env OPENTUI_FORCE_WCWIDTH=1 opencode mini --model <provider>/<model>
```

Mechanics: mllm spawns via `shell_words::split` → direct exec of `/usr/bin/env`, which sets the
variable for the child `opencode` process; OpenTUI reads it from its own `process.env`. No shell
interpretation (`&&`, `$VAR`, …) is involved, so the prefix is safe under non-shell spawning.

To later disable: **remove the `env OPENTUI_FORCE_WCWIDTH=1` prefix entirely** — setting it to
`0` does *not* disable it (presence flag).

## 5. Verification

- 4 **unit** tests cover the pane fix: query responder (×3, `src/pty.rs::tests`) and wide-glyph
  widget (×1, `src/ui.rs::tests`), counted within the 63-unit bucket. Suite total at the time of
  writing: 99/99 (63 unit + 32 integration + 4 audit regression — the regression bucket lives in
  `tests/reviewer_audit_regressions.rs` and covers the Protocol v1.3 audit findings, not the pane
  fix).
- The OSC 66 regression sample mirrors the real probe draw shape (`w=2`), with a comment
  explaining the CPR-trick detection (§3.1).
