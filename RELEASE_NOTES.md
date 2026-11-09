# Multi-LLM (`mllm`) Release Notes

---

## Release v0.5.1 (November 2026)

**Release Tag**: [`v0.5.1`](https://github.com/yeusin/mllm/releases/tag/v0.5.1)  
**Theme**: Vendored vt100 Parser & Pane-Fidelity Hardening

Full-screen TUI agents (OpenCode/OpenTUI and similar) redraw using cursor sequences that upstream `vt100 0.18.1` silently ignores: `CSI s`, `CSI u`, `ESC D` and `ESC E`. Inside mllm panes the parser's cursor diverged from a real terminal's, so client redraws landed one row too high and overwrote or erased existing lines. v0.5.1 ships the fix: a vendored `vt100 0.18.1` under `vendor/vt100/` with a minimal four-arm patch (commit `d203798`), plus the replay harness and documentation that keep it auditable (harness in `d203798`; reference doc and README pointer in `d9c9de5`). Full reference: [`docs/vendored-vt100.md`](docs/vendored-vt100.md).

### Fixes
- **Cursor save/restore**: `CSI s` / `CSI u` now save and restore the cursor (SCOSC/SCORC) in the vendored parser, so OpenTUI redraws land on the correct rows.
- **Line motion**: `ESC D` (IND) moves down and scrolls at the bottom margin like a real terminal instead of being dropped; `ESC E` (NEL) is handled for completeness.
- **PTY startup handshake**: terminal queries from TUI clients are answered (`CSI 6n` with the parser's cursor position, `OSC 10/11` foreground/background colors, XTGETTCAP declined as unsupported), so clients no longer stall or degrade while waiting on probes.
- **Wide glyphs**: the pane widget skips wide-character continuation cells, so emoji/icons render both halves intact under ratatui's diff-based redraw.

### Documentation
- **New `docs/vendored-vt100.md`**: why the parser is vendored, the patch itself, the upstream sync procedure (`diff -ru` against the local cargo registry cache; re-apply the four arms; re-run the regression tests), and the fork citation policy — when consulting the community fork, read the published `vt100-psmux 0.16.13` crate, not the stale GitHub `main` branch (and note the fork carries no IND/NEL arms).
- **README §1.1 pointer** to the new page, and corrected suite counts in `README.md` and `docs/tui-agent-notes.md`.

### Pane-fidelity verification
- **Opt-in replay harness** (no-ops without environment variables, so CI is unaffected): `REPLAY_CAPTURE` renders a raw capture through the real parser → widget → ratatui-diff path; `REPLAY_MULTIFRAME` splits a capture at synchronized-frame markers (`ESC[?2026l`) and emits the resulting host-byte stream for comparison.
- **Ground truth**: the wide-char-aware reference emulator `scripts/ref_emu.py`, with capture tooling in `scripts/capture_raw.py`, `scripts/capture_bidir.py` and `scripts/capture_session.py` (`CAPTURE_*` environment variables).
- The validating run replayed 83 synchronized frames (117×49 pane geometry) identical to ground truth with the patch applied; that frame count is not a reproducible invariant — reproduction is the loop itself: capture a session, replay it, compare against the reference emulator.

### Known limitations
- SGR 5 (blink), 8 (conceal) and 9 (strikethrough) are not rendered. SGR 8 text intended to be hidden renders **visibly**: concealed content is not protected in panes, and would also leak through selection and clipboard export today, and any future search. No captured client emits SGR 5/8/9.
- SGR 2 (dim) is parsed but not painted; dim text renders at normal intensity.
- `CSI s` is treated as save-cursor (SCOSC) unconditionally: DECLRMM (`?69`) / DECSLRM margin mode is unsupported upstream, and no mllm client sets it.
- Verification: 100 tests pass (64 unit, 32 integration, 4 audit regression), including three new vt100 patch regression tests (`test_vt100_patch_cursor_save_restore`, `test_vt100_patch_ind_nel`, `test_vt100_patch_ind_scrolls_at_bottom`). Documentation content audited line-by-line by the Reviewer.

---

## Release v0.5.0 (November 2026)

**Release Tag**: [`v0.5.0`](https://github.com/yeusin/mllm/releases/tag/v0.5.0)  
**Theme**: Mandatory Reply Logging & Watchdog Fallback (Task Dispatch & Chat Logging Protocol v1.3)

Agents sometimes finished a turn in the terminal without appending a reply to the shared chat log, leaving the coordinator waiting for the 600s UI timeout. This release enforces logging from the coordinator side. Full specification: [`docs/task-dispatch-protocol.md`](docs/task-dispatch-protocol.md).

### Features
- **Per-turn protocol suffix**: every dispatched task, not just the first, ends with a mandatory logging directive with pre-filled `reply_to`, `sender`, `role` and chat file path.
- **Idle watchdog**: a client with no screen change and no log growth gets Nudge 1 at 25s and Nudge 2 at 40s (through the same typing queue as dispatch). A 15s post-dispatch grace applies.
- **Scrape fallback**: at 55s idle (or earlier on a strict standalone prompt match) the visible screen is logged on the agent's behalf with `"fallback": true`. Fallback IDs are deterministic (UUIDv5), fallbacks are never routed to peers, and a late real reply marks the fallback as superseded in the UI.
- **Schema**: `ChatMessage` gains optional `fallback` and `superseded_by` fields. Both are omitted when unset, so existing logs and parsers keep working. `superseded_by` is in memory only; the log file is never rewritten.

### Fixes
- Salvage pass splits fused `}{` records instead of dead-lettering both.
- The coordinator now ingests pending log updates before appending its own message, so an agent line written in that gap is no longer skipped.
- The watchdog is cleared only by a reply to the tracked task, not by any message from that sender.
- Truncation of scraped text no longer risks splitting a multi-byte character.

### Known limitations
- The `mllm-reply` helper command is not yet available. Agents write the JSON line themselves; each reply must be a single complete line in one write. On network filesystems, interleaving is possible.
- Watchdog timers start at dispatch routing, not at paste completion (absorbed by the 15s grace).
- Sender matching is case-insensitive and trims surrounding whitespace; the unrecognized-sender warning is not implemented.
- Verification: 91 tests pass (55 unit, 32 integration, 4 audit regression). Implementation audited and approved by the Reviewer.

---

## Release v0.4.0 (November 2026)

**Release Tag**: [`v0.4.0`](https://github.com/yeusin/mllm/releases/tag/v0.4.0)  
**Theme**: Full-Fidelity Rich Coordinator Chat, Centralized XDG Config & Inode-Isolated History Logs

Multi-LLM v0.4.0 upgrades Pane 0 (Center Coordinator Chat) with an in-tree rich terminal renderer supporting CommonMark formatting, strict ECMA-48 SGR ANSI color decoding with visible caret sanitization, and full UTF-8 emoji display width alignment. It also introduces viewport-windowed height caching and eliminates legacy terminal startup boundary markers.

---

### Highlights & Key Features

#### 1. In-Tree Zero-Dependency Rich Terminal Renderer (`src/rich_text.rs`)
- **Rich Text Elements**: Native rendering of headers (`#`, `##`, `###`), bold (`**bold**`), italic (`*italic*`), inline code (`` `code` ``), fenced code blocks (```lang ... ```), unordered/ordered lists (`-`, `*`, `1.`), blockquotes (`>`), and horizontal dividers (`---`).
- **Strict ECMA-48 SGR ANSI Allowlist**: Decodes standard 16-color, 256-color (`\x1b[38;5;Nm`), and truecolor (`\x1b[38;2;R;G;Bm`) SGR sequences into Ratatui `Style` and `Color` attributes.
- **Visible Caret Sanitization**: Disallowed or unsafe ANSI escape sequences (OSC title spoofing, OSC 52 clipboard hijack attempts, non-SGR CSI cursor commands, and C0/C1 controls) are neutralized into human-auditable caret notation (e.g. `^[[2J`, `^[OSC...^]`) instead of executing or silently mutating.
- **Code Span Precedence**: Inline and fenced code blocks strictly suppress `@mention` highlighting, preserving literal code representation while retaining `@mention` styling across standard markdown text, headers, lists, and quotes.
- **Unicode & Emoji Width Alignment**: Full-width and multi-codepoint emoji support (`unicode-width` and `unicode-segmentation`) with grapheme cluster truncation (`…`) and display-width calculation parity in multiline composer wrapping.
- **Zero New Dependencies & Measured Binary Footprint**: Implemented in-tree without third-party markdown AST or ANSI crates (0 new external dependencies, directly declaring and reusing `unicode-segmentation 1.13.3` from `Cargo.lock`), with a measured landed binary footprint delta of +49,968 B (+49.9 KiB / +1.09%) stripped size (+48.7 KiB `.text`, +128 B `.data`).

#### 2. Scalable Viewport-Windowed Rendering & Height Caching (`src/ui.rs`, `src/app.rs`)
- **Strict $O(\text{visible})$ Scroll Scaling**: Eliminates per-frame paragraph re-wrapping by precomputing and caching message heights and total line counts on ingestion and resize.
- **Viewport Windowing**: Materializes `Vec<Line>` solely for visible viewport message bounds `[scroll_y, scroll_end)`.
- **Zero-Thrash Eviction**: Bounded cache (2,000 entries) safely evicts oldest entries without thrashing because off-screen messages are never re-materialized in the render loop.
- **Empirical Benchmarks (120x40 Terminal, `fff3ab1` Release Build)**:
  - **Single-Message Render**: Mean `15.26 µs` (p50: `14.01 µs`, p95: `23.70 µs`).
  - **Adversarial Recursion Defense**: 105,000-byte unclosed delimiter line completes in `12.54 ms` (18,001 spans, bounded $O(1)$ stack depth, zero recursion).
  - **Cold Initial Ingest**: One-time startup baseline of ~523 ms for 5,000 messages (~104.6 µs/msg) and ~984 ms for 10,000 messages (~98.4 µs/msg) to compute initial height indices; fully cached for all subsequent scroll frames.
  - **5,000 Messages Scrollback**: `1.02 ms` mean frame time (93.9% 60 FPS headroom, 37.6x speedup over uncached baseline).
  - **10,000 Messages Scrollback**: `1.01 ms` mean frame time (94.0% 60 FPS headroom, 93.8x speedup over uncached baseline, 0.00 ms growth over 5,000 messages).

#### 3. In-Memory Session Boundary Tracking (`src/app.rs`, `src/chat.rs`)
- **Zero PTY Stdin Buffer Pollution**: Deprecated legacy `BOUNDARY_MARKER` string emission to child PTY stdin.
- **Programmatic Filtering**: Session boundary tracked exclusively via in-memory `initial_message_ids: HashSet<String>`, cleanly filtering historical records from autonomous dispatch while rendering history in the Coordinator UI.

#### 4. Centralized XDG Configuration, Partitioned History & Legacy Deprecation (`src/config.rs`, `src/main.rs`)
- **Centralized XDG Config**: Multi-LLM defaults to `$HOME/.config/mllm/mllm.yaml` with automatic first-launch scaffolding and 0700/0600 file permission hardening.
- **Device & Inode Isolated History**: Conversation event logs are partitioned per working directory filesystem device and inode at `$HOME/.config/mllm/history/{dev}_{ino}/chat.jsonl` with zero cross-mount collision risk.
- **Fixed Internal History Path**: Removed `chat_file` configuration from `mllm.yaml`; runtime paths are managed internally and interpolated dynamically into system instructions.
- **Alt Prefix Navigation**: Adopted `alt` / `alt+<key>` prefix chords with direct unprefix navigation.
- **Streamlined CLI**: Removed `mllm init` in favor of auto-scaffolding; eliminated legacy compatibility layers (`ctrl+b`, `{working_dir_inode}`, variable aliases, and cwd auto-migration).

---

### Verification & Quality Gates
- **Unit & Integration Tests**: 83 passed / 0 failed (55 unit + 28 integration) including rich text parsing, mention routing, and cross-mount collision isolation.
- **Compiler Hardening**: `#![deny(clippy::indexing_slicing)]` and `#![deny(clippy::string_slice)]` enforced across `src/rich_text.rs`.
- **Lint Audit Attribution**: Confirmed 35 pre-existing compiler/clippy lints under Rust 1.96.0 toolchain drift (31 `collapsible_if`, 1 bool literal, 1 `div_ceil`, 1 const static lifetime, 1 clamp pattern) with zero regressions or new lint classes introduced by `eaed037` and `fff3ab1`. Scheduled for dedicated maintenance pass.

---

## Release v0.3.0 (November 2026)

**Theme**: Deterministic Agent Working Directory  
**Release Tag**: [`v0.3.0`](https://github.com/yeusin/mllm/releases/tag/v0.3.0)  
**Commit**: `61fc832`

Multi-LLM v0.3.0 is a focused behavioral fix release ensuring that agent client processes always launch in the directory from which `mllm` was invoked.

---

### Fixes

#### Agents Launch in the Invocation Working Directory
Previously, client processes were spawned without an explicit working directory, relying on inherited cwd semantics that could leave agents starting in an unpredictable default location (e.g., the home directory) instead of the project directory the operator was working in.

- **Explicit PTY Working Directory**: `ClientPane::new` now accepts a `working_dir` argument and pins it on the spawned PTY command via `CommandBuilder::cwd`, making the agent's starting directory deterministic across platforms and launch contexts.
- **Invocation Directory Resolution**: `AppState::new` resolves `mllm`'s current working directory at startup and passes it to every configured client pane, so all agents share the same working directory as the coordinator session.

---

## Release v0.2.0 (November 2026)

**Theme**: Autonomous Self-Update, Atomic Lazy Injection & Instruction Templating  
**Release Tag**: [`v0.2.0`](https://github.com/yeusin/mllm/releases/tag/v0.2.0)  
**Commit**: `8e5426b`  

Multi-LLM v0.2.0 is a major milestone release focusing on seamless application lifecycle management, multi-agent instruction ergonomics, and token context efficiency. It introduces native in-place self-updates via `mllm update`, dynamic configuration variable interpolation, and atomic lazy instruction injection that completely eliminates startup chat file pollution.

---

### Highlights & Key Features

#### 1. Native Self-Update Command (`mllm update`)
Multi-LLM now provides a built-in, unassisted upgrade mechanism that automatically discovers the host target architecture, queries GitHub Releases for the latest or specified version, validates transport integrity via SHA-256 checksums, and safely replaces the running executable in place.

- **Zero-Bloat Subprocess Architecture**: Avoids adding heavy runtime networking or cryptographic dependencies (`reqwest`, `hyper`, `rustls`, `ring`, or `self_update`). Subprocess orchestration via native OS utilities (`curl`/`wget`, `tar`, `sha256sum`/`shasum`/`openssl`) preserves lightweight binary footprint (~16.5 KB compiled text footprint, ~0.3% of total binary size) and maintains sub-6-second release compile times.
- **Atomic In-Place Replacement**: Eliminates Unix `ETXTBSY` (Text file busy) execution lockups by staging the downloaded executable into `.mllm-update-<uuid>.tmp` within the same target directory, applying `0755` executable permissions, executing `sync_all()` (`fsync`) for disk durability, and performing an atomic POSIX `rename(2)` over the target path.
- **Fail-Closed SHA-256 Checksum Verification**: Mandates transport checksum verification before extracting any archive. Validates against the official `mllm-<target>.sha256` release asset (with `install.sh` also verifying `.tar.gz.sha256` naming fallbacks). Mismatched or unretrievable checksums halt immediately before altering files.
- **Hardened Multi-User Staging**: Extracts archives into a private temporary directory restricted to `0700` (`DirBuilderExt::mode(0o700)`) and invokes `tar` with `--no-same-owner --no-same-permissions` to eliminate privilege escalation hazards during `sudo` upgrades. Extracted payloads are validated as non-symlink regular files.
- **Flexible CLI Controls**:
  - `mllm update --check` (`-c`): Inspect available updates against the local version without downloading or writing files.
  - `mllm update --version <VER>` (`-v`): Pin to an exact release tag (e.g. `v0.1.0`) or switch versions on demand.
  - `mllm update --force` (`-f`): Force reinstallation or binary refresh of the current version.
  - `mllm update --dry-run`: Preview resolved platform target, download URLs, checksum URLs, and atomic replacement targets without filesystem side-effects.
  - `mllm update --target <PATH>` (`-t`): Explicitly target user-local installations (e.g. `~/.local/bin/mllm`) or system paths (`/usr/local/bin/mllm`).
- **Intentional Downgrade Support & Semantic Versioning**: Validates version strings against semantic versioning schemas. If an operator requests an older release (e.g. rollback to `v0.1.0`), `mllm update` prints an explicit warning notice (`Notice: Target version ... is older than current version ... (downgrade)`) while keeping execution unblocked.

#### 2. Dynamic Instruction Template Variables
Configuration preambles and role instructions now support dynamic placeholders that are interpolated during runtime execution. This ensures agent system prompts always reflect runtime settings without hardcoded path or configuration duplication.

- **Supported Variables**:
  - `{chat_file}` / `{chatfile}`: Resolves to the configured shared JSONL event log path.
  - `{prefix_key}` / `{prefixkey}`: Resolves to the configured prefix key combination (e.g., `ctrl+b`).
  - `{name}` / `{client_name}`: Resolves to the specific agent's identifier (e.g., `Architect`).
  - `{role}` / `{client_role}`: Resolves to the agent's assigned role (e.g., `Lead Architect`).
  - `{command}` / `{client_command}`: Resolves to the client's executed binary (e.g., `agy`).
  - `{number}` / `{client_number}`: Resolves to the client's pane index (e.g., `1`).
- **Safe Parsing**: Supports whitespace flexibility (`{chat_file}` and `{ chat_file }`) while safely ignoring non-variable curly brace syntax like raw JSON schemas (`{"id": "<uuid>"}`).

#### 3. Unified Atomic Lazy Instruction Injection
Previous versions broadcast startup instructions directly into `shared_chat.jsonl` upon cold start, resulting in artificial coordinator chat entries, staggered input replays, and unnecessary token context consumption before any real tasks were assigned.

- **Zero Startup Chat Pollution**: The shared chat file remains 100% clean of synthetic coordinator initialization broadcasts.
- **Atomic Composite Prompting**: General protocols and role-specific instructions are held in memory and lazily prepended to the agent's **very first assigned task prompt**:
  ```text
  [General Instructions & Protocol]
  <Interpolated General Protocol>

  [Role Instructions]
  Name: <Name>
  Role: <Role>
  <Interpolated Role Specifics>

  Task from <Sender> (<Role>): <Task Content> [Ref: <UUID>]
  ```
- **Context Window Optimization**: Once an agent has executed its initial composite prompt, subsequent tasks are dispatched cleanly with only the task payload. This eliminates redundant instruction overhead across autonomous multi-turn loops.
- **Consistent Cold and Warm Starts**: Operates identically whether starting with an empty log or resuming an existing session.

---

### Security, Reliability & Performance Audit

| Area | Implementation & Assurance |
| :--- | :--- |
| **Atomic File Replacement** | POSIX `rename(2)` from `.mllm-update-<uuid>.tmp` guarantees that the running executable is never truncated while active (`ETXTBSY` avoidance). |
| **Disk Durability** | Staged binaries invoke `fsync` (`sync_all()`) before replacement, protecting against power-loss corruption. |
| **Transport Integrity** | SHA-256 hash calculation occurs before decompression; missing or diverging checksums halt execution with non-zero exit codes. Checksums protect against network/download corruption, not upstream infrastructure compromise. |
| **Temporary Staging Sandbox** | Temporary directories use `0700` mode, preventing multi-user `/tmp` race conditions or symbolic link hijacks. |
| **Extraction Hardening** | Invocations of `tar` enforce `--no-same-owner --no-same-permissions` to eliminate privilege escalation hazards when running under `sudo`. |
| **Subprocess Efficiency** | Memory consumption during updates remains under **5 MB peak RSS**, eliminating OpenSSL runtime linking conflicts. |

> [!NOTE]
> Releases in v0.2.0 and prior rely strictly on published SHA-256 transport checksums and are unsigned. Cryptographic build provenance attestations are introduced in v0.2.1.

---

### Test Suite Verification

Multi-LLM v0.2.0 expands test coverage with 29 new tests across the update engine, variable interpolation, and lazy prompt injection:

- **Total Tests**: **67 passed; 0 failed; 0 ignored**
  - **44 Unit Tests** (`src/lib.rs`):
    - `update::tests::test_detect_target_triple`
    - `update::tests::test_normalize_version`
    - `update::tests::test_dry_run_output`
    - `update::tests::test_check_up_to_date_output`
    - `update::tests::test_atomic_replace_and_permissions`
    - `update::tests::test_parse_semver_tuple_and_downgrade_notice`
    - `update::tests::test_target_directory_rejected_early`
    - `update::tests::test_validate_version_string`
    - `update::tests::test_checksum_verification_fail_paths`
    - `update::tests::test_extracted_payload_validation_fail_paths`
    - `update::tests::test_broken_symlink_target_handling`
    - `chat::tests::test_instruction_placeholders_replacement`
    - `chat::tests::test_build_startup_instructions_with_vars`
  - **23 Integration Tests** (`tests/integration_tests.rs`):
    - `test_atomic_composite_prompt_lazy_injection`
    - `test_update_workflow_dry_run_and_check`
    - `test_update_atomic_replacement_integrity`
    - `test_startup_instructions_playback_on_app_init`

---

### Upgrading to v0.2.0

#### Using Self-Update (Recommended)
If you already have `mllm` v0.1.0 or later installed:
```bash
mllm update
```
To update an installation located in system directories (`/usr/local/bin`):
```bash
sudo mllm update
```

#### Using the Automated Installer
```bash
curl -fsSL https://raw.githubusercontent.com/yeusin/mllm/main/install.sh | bash
```

#### Building from Source
```bash
git clone https://github.com/yeusin/mllm.git
cd mllm
git checkout v0.2.0
cargo install --path .
```

---

## Upcoming v0.2.1 Supply-Chain & Hardening Roadmap

The upcoming **v0.2.1** release introduces cryptographic supply-chain attestations and deepened staging isolation:

1. **Cryptographic SLSA Build Provenance Attestations**:
   - Integration with GitHub Artifact Attestations (`actions/attest-build-provenance@v2`) generating Sigstore-backed DSSE/in-toto SLSA v1.0 provenance for all release artifacts.
   - **Non-Retroactivity Boundary**: SLSA attestations apply exclusively starting with **v0.2.1**; v0.2.0 and prior releases rely strictly on published SHA-256 checksums.
   - **Verification Workflow (`gh >= 2.43`)**:
     ```bash
     gh attestation verify mllm-<target>.tar.gz \
       --repo yeusin/mllm \
       --signer-workflow yeusin/mllm/.github/workflows/release.yml
     ```
   - **Zero Added Footprint**: Attestation verification runs out-of-band via GitHub CLI, keeping `mllm` at 0 added dependencies and ~16.5 KB `.text` footprint.
2. **File Staging Hardening**:
   - Upgrading `.mllm-update-<uuid>.tmp` creation to `std::fs::File::create_new` (`O_CREAT | O_EXCL`) via `OpenOptionsExt`, reinforcing atomic replacement against pre-existing symlinks or concurrent collisions.
3. **Action Commit SHA Pinning**:
   - Pinning all GitHub Action references in `.github/workflows/release.yml` to full 40-character hex commit hashes with semantic version annotations.

---

## Release v0.1.0 (November 2026)

**Initial Public Release**
- High-performance, tmux-free terminal multiplexer built in Rust with `portable-pty` and `vt100`.
- Symmetrical multi-agent layout arranging client PTYs evenly around the center Coordinator pane.
- Atomic append-only JSONL shared event bus (`shared_chat.jsonl`) with `O(1)` byte-offset polling.
- Real-time LLM status indicators, bracketed paste prompt injection, and autonomous `@mention` routing with a 100-turn safety circuit breaker.
- Cross-platform installer script (`install.sh`) supporting Linux and macOS (x86_64, aarch64, and Apple Universal binary).
