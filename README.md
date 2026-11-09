# Multi-LLM (`mllm`)

Multi-LLM is a Rust-based terminal multiplexer and multi-agent orchestration TUI. It provides tmux-style multi-pane layouts without tmux dependencies, running interactive LLM clients in isolated PTYs. Agents seamlessly collaborate in real time via atomic JSONL event logging and automated keystroke injection.

---

## 1. System Architecture & Symmetrical Layout

### 1.1 Architecture Overview

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│                                 Multi-LLM TUI                                    │
├──────────────────────────┬───────────────────────────┬───────────────────────────┤
│  Pane 1: Architect       │  Pane 0: Coordinator Chat │  Pane 4: TechnicalWriter  │
│  [Role: Lead Architect]  │  [Role: Coordinator]      │  [Technical Writer]       │
│  (PTY: agy)              │  (Shared JSONL Stream)    │  (PTY: agy)               │
│                          │                           │                           │
│  > Bracketed paste prompt│  [#e1a2] User: Build feature├───────────────────────────┤
│  > Interactive PTY session│  [#f3b4] Architect: Plan  │  Pane 5: Designer         │
│  > Isolated mouse select │  [#a5c6] Reviewer: Audit  │  [Brand & UI/UX]          │
├──────────────────────────┤  [#d7e8] Researcher: Bench│  (PTY: agy)               │
│  Pane 2: Reviewer        │                           ├───────────────────────────┤
│  [Reviewer & Auditor]    │                           │  Pane 6: Marketer         │
│  (PTY: agy)              │                           │  [Growth & Campaign]      │
├──────────────────────────┤                           │  (PTY: agy)               │
│  Pane 3: Researcher      │  [Composer: @Architect ]  │                           │
│  [Role: Researcher]      │  [Tab: Cycle Responder]   │                           │
├──────────────────────────┴───────────────────────────┴───────────────────────────┤
│ 0:Coordinator* 1:Architect 2:Reviewer 3:Researcher [Prefix: ALT] [COLLABORATE]   │
└──────────────────────────────────────────────────────────────────────────────────┘
```

> **Vendored vt100 parser**: mllm vendors `vt100 0.18.1` under `vendor/vt100/` with a minimal
> four-arm patch (cursor save/restore `CSI s`/`u`, IND/NEL `ESC D`/`E`) that upstream lacks and
> OpenTUI-driven TUIs require — without it, panes redraw on wrong rows. See
> [docs/vendored-vt100.md](docs/vendored-vt100.md) for rationale, upstream sync, the pane-fidelity replay harness, and known limitations.

### 1.2 Layout Rules & Symmetrical Partitioning

Multi-LLM dynamically and symmetrically arranges arbitrary client panes around the central Coordinator without requiring pre-set window templates:

1. **Center Coordinator Pane (`Pane 0`)**:
   - Strictly reserved for the **Center Coordinator** (Name: `Coordinator`, Role: `Coordinator`).
   - Occupies the center column (`Constraint::Ratio(12, 32)` or ~38% screen width, 100% height).
   - Features the live rich-text chat stream (inline/block Markdown, code blocks, ANSI colors, UTF-8 emojis), threading indicators, sender color palettes, status banners, and the multiline text composer.
2. **Left and Right Side Columns (`Pane 1 .. Pane N`)**:
   - Active client panes are distributed evenly around the center column:
     - `mid = client_count / 2`
     - **Left Column** (`Constraint::Ratio(10, 32)`): Panes `0 .. mid` (e.g. Panes 1, 2, 3).
     - **Right Column** (`Constraint::Ratio(10, 32)`): Panes `mid .. N` (e.g. Panes 4, 5, 6).
   - Panes within each column share vertical space equally (`Constraint::Ratio(1, column_client_count)`).
3. **Adaptive Symmetry Across Client Counts**:
   - $N = 1$: Left [Pane 1], Center [Pane 0: Coordinator].
   - $N = 2$: Left [Pane 1], Center [Pane 0: Coordinator], Right [Pane 2].
   - $N = 3$: Left [Pane 1: Architect], Center [Pane 0: Coordinator], Right [Pane 2: Reviewer, Pane 3: Researcher].
   - $N = 4$: Left [Pane 1, Pane 2], Center [Pane 0: Coordinator], Right [Pane 3, Pane 4].
   - $N = 6$: Left [Pane 1, Pane 2, Pane 3], Center [Pane 0: Coordinator], Right [Pane 4, Pane 5, Pane 6].
4. **Status Bar**:
   - Classic tmux-style status line docked at the bottom (`Constraint::Length(1)`).
   - Displays session name, numbered pane tabs with active indicators (`0:Coordinator*`), the prefix indicator (`[PREFIX]` highlighted when armed, otherwise `[Prefix: ALT]`), and the mode badge (`[COLLABORATE]` on the Coordinator pane, `[DIRECT]` on client panes).

---

## 2. Configuration Specification (`$HOME/.config/mllm/mllm.yaml`)

Configuration is centralized at `$HOME/.config/mllm/mllm.yaml` (automatically scaffolded on first launch if not present), or overridden via `--config <path>` (`-c <path>`). Alternatively, an example template is available at `mllm.yaml.example`.

### 2.1 YAML Schema & Multi-Agent Example

```yaml
# Multi-LLM Configuration ($HOME/.config/mllm/mllm.yaml)
prefix_key: "alt"

# General instructions shared across all LLM clients
general_instructions: |
  Shared Log Coordination & Output Protocol:
  1. Shared Log File: All clients coordinate and store outputs by appending to {chat_file}.
  2. Output Storage in Shared Log: Write each output as a single valid JSON line followed by a newline:
     {"id": "<uuid>", "reply_to": "<question_uuid_or_null>", "sender": "<YourName>", "role": "<YourRole>", "message": "<Content with optional @Peer>", "timestamp": <unix_timestamp>}
  3. Unique UUID & Question Linking: Generate a unique UUID for "id" and reference the question's UUID with "reply_to".
  4. Unix Timestamp: The "timestamp" field must be an integer Unix timestamp in epoch seconds.
  5. Directing Tasks & Mention Format:
     - Header Mentions: Place target mentions at the start of a line or message (e.g. '@Reviewer @Researcher: <task>' or '@User @Reviewer: <update>').
     - Multiple Targets: Multiple leading mentions on the same line are supported. Mentioning '@User' addresses the human operator without blocking peer dispatch.
     - Directive Bullet Points: Directives formatted as markdown list items (e.g. '- To @Reviewer: <task>' or '- @Reviewer: <task>') are automatically extracted and routed.
     - Broadcast: Mention '@all' at the start of a line to broadcast a task to all available peer agents.
     - Mid-Sentence Mentions: Mentions embedded in casual mid-sentence prose (e.g. 'coordinated with @Reviewer') are ignored to prevent accidental routing loops.
  6. Mention Routing: The Center Coordinator monitors the shared log and automatically delivers @all or @mentioned tasks to target agents.
  7. Communication Standards: Be concise, clear, and actionable. Rich terminal formatting (inline & block Markdown, code blocks, ANSI colors, and emojis) is natively supported in the Center Coordinator chat.
  8. Mandatory Result Output: Always output results and answers by appending to {chat_file}. If an agent does not write its output to {chat_file}, the coordinator keeps showing the agent as in-progress.

clients:
  - number: 1
    name: Architect
    role: Lead Architect
    command: agy --dangerously-skip-permissions
    instructions: |
      Lead system architecture, technical specifications, runtime decisions, and interface contracts.
      Evaluate system language choices, concurrency patterns, and framework tradeoffs.
      Direct security, edge cases, and audit concerns to @Reviewer in the shared log.
      Direct ecosystem, crate, and benchmark investigations to @Researcher in the shared log.

  - number: 2
    name: Reviewer
    role: Reviewer & Auditor
    command: agy --dangerously-skip-permissions
    instructions: |
      Review architecture, code quality, edge cases, vulnerability vectors, and concurrency safety.
      Audit reliability, error handling, memory safety, and maintenance overhead.
      Direct architectural redesign questions to @Architect in the shared log.
      Direct benchmark verification requests to @Researcher in the shared log.

  - number: 3
    name: Researcher
    role: Researcher
    command: agy --dangerously-skip-permissions
    instructions: |
      Conduct technical research, performance profiling, library evaluations, and ecosystem comparisons.
      Analyze cross-platform tooling, crate/package benchmarks, and developer velocity tradeoffs.
      Direct architectural implications to @Architect in the shared log.
      Direct compliance and security findings to @Reviewer in the shared log.

  - number: 4
    name: TechnicalWriter
    role: Technical Writer
    command: agy --dangerously-skip-permissions
    instructions: |
      Author and maintain technical documentation, specifications, user guides, API docs, and release notes.
      Ensure clarity, accuracy, completeness, and structured formatting across all project documentation.
      Direct architectural clarification requests to @Architect in the shared log.
      Direct accuracy and implementation verification queries to @Reviewer in the shared log.

  - number: 5
    name: Designer
    role: Brand & UI/UX Designer
    command: agy --dangerously-skip-permissions
    instructions: |
      Create design system components, wireframes, user journeys, and visual layouts.
      Direct documentation layout and copy alignment requests to @TechnicalWriter.
      Direct campaign asset specifications to @Marketer.

  - number: 6
    name: Marketer
    role: Growth & Campaign Specialist
    command: agy --dangerously-skip-permissions
    instructions: |
      Evaluate user acquisition channels, audience segmentation, conversion funnels, and KPIs.
      Direct technical content and messaging adjustments to @TechnicalWriter.
      Direct banner, landing page, and visual requirements to @Designer.
```

### 2.2 Configuration Fields
- `prefix_key` *(string, default: `"alt"`)*: Prefix combination (`"alt"` or `"alt+<key>"`).
- `general_instructions` *(string)*: Protocol instructions injected into each client's prompt context.
- `clients` *(array)*:
  - `number` *(usize, 1..9)*: Unique pane number (`0` is reserved for Coordinator; duplicates auto-reassign).
  - `name` *(string)*: Identifier for `@mention` routing (case-insensitive).
  - `role` *(string)*: Role description displayed in the pane header.
  - `command` *(string, default: `"agy"`)*: Command spawned in the PTY. Supports shell arguments via `shell-words` (e.g. `agy --dangerously-skip-permissions`).
  - `instructions` *(string)*: Persona and domain instructions injected on client initialization.
  - `extra_args` *(array of strings, optional)*: Additional arguments appended to the spawned command.

#### Dynamic Instruction Template Variables
Both `general_instructions` and client `instructions` support dynamic placeholder interpolation at runtime. This avoids hardcoding paths, role names, or shortcuts across configuration sections:

| Variable | Description |
| :--- | :--- |
| `{chat_file}` | Interpolates the resolved absolute shared event log path (e.g., `/home/user/.config/mllm/history/64512_4070304/chat.jsonl`). |
| `{prefix_key}` | Interpolates the configured prefix key (e.g., `alt`). |
| `{name}` | Interpolates the specific client agent identifier (e.g., `Architect`). |
| `{role}` | Interpolates the agent's assigned role description (e.g., `Lead Architect`). |
| `{command}` | Interpolates the executable binary name (e.g., `agy`). |
| `{number}` | Interpolates the client pane index number (e.g., `1`). |

> [!NOTE]
> Placeholder matching supports internal spacing (e.g. `{ chat_file }`) and safely preserves unmatched curly braces and raw JSON schemas (such as `{"id": "<uuid>"}`).

An example configuration is provided in `mllm.yaml.example`. Copy it to `mllm.yaml` to customize your multi-agent environment.

---

## 3. Shared JSONL Protocol & Coordination Engine

### 3.1 JSONL Record Schema

Every message exchanged across users, coordinator, or autonomous agents is an atomic single-line JSON record:

```json
{"id": "a1b2c3d4-e5f6-7a8b-9c0d-1e2f3a4b5c6d", "reply_to": null, "sender": "User", "role": "Human Operator", "message": "@Architect evaluate async runtime", "timestamp": 1794053618}
{"id": "b2c3d4e5-f6a7-8b9c-0d1e-2f3a4b5c6d7e", "reply_to": "a1b2c3d4-e5f6-7a8b-9c0d-1e2f3a4b5c6d", "sender": "Architect", "role": "Lead Architect", "message": "Tokio selected. @Reviewer audit safety vectors.", "timestamp": 1794053630}
```

| Field | Type | Description |
| :--- | :--- | :--- |
| `id` | `String` (UUIDv4) | Unique UUID identifying the message record. |
| `reply_to` | `Option<String>` | UUID of the triggering message or task thread being responded to. |
| `sender` | `String` | Author identifier (`"User"`, `"Coordinator"`, or agent name e.g. `"Architect"`). |
| `role` | `String` | Role designation (`"Human Operator"`, `"Coordinator"`, `"Lead Architect"`). |
| `message` | `String` | Message content, optionally directing tasks via leading `@mention` tags. |
| `timestamp` | `i64` | Integer Unix timestamp in epoch seconds (rendered locally as `YYYY-MM-DD HH:MM:SS`). |

### 3.2 Mention Routing & Choreography
1. **Leading Mention Extraction**:
   - Leading `@<AgentName>` mentions (e.g., `@Architect`, `@Reviewer`) route the task directly to that specific agent.
   - `@all` broadcasts the prompt concurrently to all registered agents.
   - Mention tags are stripped from context payloads (`strip_at_mentions`) to prevent recursive accidental loops.
2. **Forced Responder & Default Fallback**:
   - If a prompt has no mention, the composer defaults to the earliest available agent (`@Architect`).
   - If user submits without a mention, it defaults to the configured earliest responder.
3. **Chain Depth Safety Circuit Breaker (`max_chain_depth = 100`)**:
   - Each autonomous peer-to-peer turn increments `active_chain_depth`.
   - Upon reaching 100 turns, autonomous dispatch pauses and emits a notification:  
     `"Chain depth limit reached (100). Pausing autonomous agent turns."`
   - Reset: Any manual submission from the human operator resets `active_chain_depth = 0`, granting a fresh 100-turn budget.
4. **In-Memory Session Startup Boundary**:
   - Historical records are loaded on startup into `initial_message_ids` (`HashSet<String>`).
   - Historical messages are tracked internally to exclude pre-existing log events from autonomous dispatch (`!initial_message_ids.contains(&msg.id)`), without injecting synthetic markers or polluting client PTY stdin buffers.
   - Autonomous reactions only fire on newly appended log events.

### 3.3 Mention Syntax Examples
```text
@Architect: Please design the database schema.
@Reviewer @Researcher: Please evaluate the proposed cryptographic scheme.
- To @Reviewer: Verify the error handling in lib.rs.
- @TechnicalWriter: Update the README with new CLI flags.
@all: Release v0.2.0 is live. Stand by for team status updates.
```
Mentions embedded in prose (e.g. `we met with @Architect earlier`) are ignored. A user message without any `@mention` is routed to the earliest available agent (`@Architect` by default).

### 3.4 Coordinator Chat Stream & Multiline Composer
- **Stream View**: Chronological events with local timestamps (`YYYY-MM-DD HH:MM:SS`), sender role tags, short UUIDs (`[#e1a2]`), and rich terminal formatting.
- **Composer**:
  - `Enter`: Send the message to the shared event stream.
  - `Shift+Enter`, `Alt+Enter`, `Ctrl+Enter`, or `Ctrl+j`: Insert a newline.
  - `Tab`: Auto-complete `@agent` mentions or cycle the active target responder.
  - `Esc`: Unfocus the composer.
- **Thinking Cards**: When a task is dispatched to an agent, an animated card appears in the Coordinator status banner with a spinner and elapsed time (`⠋ THINKING (14.2s)`). The card clears when the agent posts its reply to the chat log.

### 3.5 Atomic Lazy Instruction Injection & Context Optimization
Multi-LLM v0.2.0 replaces cold-start chat broadcast replay with **Atomic Lazy Instruction Injection**:

1. **Zero Event-Log Pollution**: Startup instructions are never written as synthetic rows into `shared_chat.jsonl`. The shared chat stream contains exclusively genuine human-operator and autonomous peer communications.
2. **Deferred Composite Prompting**: Agents remain uninitialized in memory until the Center Coordinator routes their very first task prompt.
3. **Atomic Composite Injection**: On that initial assignment, the system synthesizes and injects an atomic composite prompt combining interpolated general protocol, role instructions, and the task payload:
   ```text
   [General Instructions & Protocol]
   <Interpolated General Protocol>

   [Role Instructions]
   Name: <AgentName>
   Role: <AgentRole>
   <Interpolated Role Specifics>

   Task from <Sender> (<Role>): <Content> [Ref: <UUID>]
   ```
4. **Token Context Efficiency**: Subsequent task prompts routed to the initialized agent deliver only the direct task content (`Task from ...`), conserving critical LLM context window tokens and eliminating redundant instruction reprocessing.
5. **Cold & Warm Start Parity**: Guarantees identical, predictable agent initialization behavior whether starting from an empty event log or resuming a long-running multi-agent session.

---

### 3.6 Mandatory Reply Logging & Watchdog

Every dispatched task ends with a `[MANDATORY PROTOCOL]` suffix with pre-filled `reply_to`, `sender`, `role` and chat file path. A turn is complete only when a reply line with a matching `reply_to` and `sender` is appended to the shared log; terminal output alone is ignored.

If a client goes idle without logging, the coordinator escalates (idle means no screen change and no log growth):

| Idle time | Action |
| :-- | :-- |
| 25s | Nudge 1 |
| 40s | Nudge 2 (final notice) |
| 55s | Scrape fallback |

The scrape fallback copies the client's visible screen into the log as a message with `"fallback": true`. It uses a deterministic ID so duplicates collapse, and it is never routed to peers. If the real reply arrives later, the fallback is collapsed in the UI. Agents should write each reply as one complete line in a single write. See [docs/task-dispatch-protocol.md](docs/task-dispatch-protocol.md) for the full specification. The `mllm-reply` helper command is planned and is not yet available.

## 4. PTY & Terminal Emulation Engine

### 4.1 Cross-Platform PTY Subsystem
- Child processes are spawned in isolated pseudo-terminal pairs via `portable-pty`.
- Each client runs inside its own process group with `TERM=xterm-256color`, `COLUMNS=<cols>`, `LINES=<rows>`.
- Non-blocking master file descriptors pipe output into a `vt100` parser with 1,000-line scrollback buffer capacity.

### 4.2 Instant Bracketed Paste Injection
- Automated prompt injection utilizes terminal **bracketed paste** sequences:
  ```
  \x1b[200~<prompt_content>\x1b[201~\r
  ```
- **Advantages**:
  - Delivers entire multiline prompts atomically in a single flush.
  - Prevents premature submission caused by internal newlines.
  - Suppresses symbol completion modals (e.g., `@` triggers in `agy`).
  - Eliminates character-by-character typing latency.

### 4.3 Process Supervision & Crash Detection
- `ClientPane::check_status` polls `child.try_wait()` on every event tick.
- If a child process crashes or terminates:
  - Header displays an `[EXITED: <code>]` badge.
  - Pane border highlights in distinct red (`Color::Red`).
  - Reader threads exit cleanly without blocking the async runtime.

### 4.4 Window Size Synchronization
- Terminal resize events (`Event::Resize(cols, rows)`) immediately calculate new pane bounds and send `PtySize` updates to master PTYs, synchronizing child process geometry (`TIOCSWINSZ`).

---

## 5. Navigation, Keybindings & Controls

### 5.1 Keyboard Keybindings

| Shortcut | Scope | Action | Description |
| :--- | :--- | :--- | :--- |
| `<prefix>` (default `Alt`) | Global | **Arm Prefix** | Arms prefix mode; status bar highlights `[PREFIX]`. |
| `<prefix>` then `0` | Global | **Focus Coordinator** | Focuses Center Coordinator chat and composer input. |
| `<prefix>` then `1`–`9` | Global | **Focus Client Pane** | Switches focus directly to Client Pane 1 through 9. |
| `<prefix>` then `o` / `n` | Global | **Next Pane** | Cycles focus to next pane numerically. |
| `<prefix>` then `p` | Global | **Previous Pane** | Cycles focus to previous pane numerically. |
| `<prefix>` then `y` | Global | **Copy Selection** | Copies active text selection to system clipboard. |
| `<prefix>` then `Escape` | Global | **Cancel Prefix** | Disarms prefix mode. |
| `Alt+0`–`Alt+9` | Global | **Instant Jump** | Switches instantly to Pane 0–9 from anywhere (even while typing). |
| `Tab` | Composer | **Cycle Responder** | Cycles leading mention responder (`@Architect` $\rightarrow$ `@Reviewer` $\rightarrow$ `@Researcher` $\rightarrow$ `@all`). |
| `Shift+Tab` | Composer | **Reverse Responder** | Cycles leading mention responder in reverse order. |
| `Tab` / `Shift+Tab` | Navigation | **Cycle Panes** | Cycles focus forward / backward across active panes when composer is unfocused. |
| `PageUp` | Coordinator | **Scroll Chat Up** | Scrolls Center Coordinator message history up by 10 lines. |
| `PageDown` | Coordinator | **Scroll Chat Down** | Scrolls Center Coordinator message history down by 10 lines. |
| `PageUp` / `PageDown` | Client Pane | **Scroll PTY Buffer** | Scrolls focused client PTY terminal history up / down. |
| `0`–`9` | Navigation | **Direct Jump** | Direct pane switch when input is unfocused. |
| `Escape` | Input | **Unfocus / Reset** | Unfocuses composer, clears active mouse selection, and resets scrollback. |
| `Ctrl+c` | Global | **Conditional Copy** | Copies highlighted text when an active mouse selection exists (instead of quitting). |
| `Ctrl+c` / `Ctrl+q` | Global | **Clean Exit** | Terminates all child PTYs and cleanly restores terminal state (when no selection is active). |

### 5.2 Mouse Controls & Clipboard Export
- **Pane-Isolated Selection**: Left-click and drag inside any pane selects terminal text. Selection boundaries are strictly clamped to the active pane's inner rectangle, preventing text bleed across adjacent columns.
- **Automatic Clipboard Export (OSC 52)**: On mouse release (`MouseEventKind::Up`), selected text is automatically copied to the system clipboard via standard OSC 52 ANSI escape sequences (`\x1b]52;c;<base64>\x07`).
- **Click to Focus**: Left-clicking anywhere inside a pane activates and focuses it.
- **Clipboard**: Copying writes the system clipboard natively via the `arboard` crate and additionally emits OSC 52 for terminal-level clipboard support.
- **Manual Copy**: Press `Ctrl+c` (with an active selection), `<prefix>` then `y`, or right-click to copy highlighted text.
- **Mouse Wheel Scrollback**: Scrolling the mouse wheel up or down over any pane smoothly scrolls that pane's history buffer by 3 lines per notch.

---

## 6. Concurrency Safety, Performance & Reliability

Multi-LLM includes comprehensive architectural safeguards to guarantee high-throughput, crash-resilient operation:

### 6.1 O(1) Incremental Byte-Offset Polling
- **Problem**: Full-file re-parsing every tick causes $O(N)$ heap allocations (at 10,000 messages: 12.6 MB churn and 70k allocations per tick).
- **Solution**: `load_new_messages_from_offset` tracks `last_chat_offset: u64` and seeks directly via `file.seek(SeekFrom::Start(offset))`.
- **Performance**: Polling latency is ~2.3 µs at EOF with zero garbage collection overhead (a 4,270x latency improvement). Automatically resets to 0 if log rotation or truncation is detected.

### 6.2 Atomic File Locking & RAII Safety Guard
- **Problem**: Concurrent appends from independent child processes or scripts can result in interleaved, torn JSON lines under high contention.
- **Solution**: Contiguous payload assembly in memory (`Vec<u8>`) flushed under an advisory lock (`libc::flock(LOCK_EX)`).
- **RAII `FileLockGuard`**: Wraps the file descriptor by value. Automatically releases locks (`flock(LOCK_UN)`) on any error return (`?`) or unwind, eliminating descriptor lock leaks.

### 6.3 EOF Partial Record Protection
- When reading newly appended bytes, `line_buf.ends_with('\n')` ensures that partial writes mid-flush are not parsed prematurely.
- Incomplete trailing buffers remain uncommitted until the terminating newline is flushed on the next tick, preventing corrupted JSON parse errors.

### 6.4 Dead-Letter Log Queue
- Malformed JSON records ending in newlines are quarantined to a sibling file of the chat log with a `.dead_letter.jsonl` extension (e.g. `chat.dead_letter.jsonl`), and processing continues with subsequent lines.
- The shared log can be safely truncated or rotated while Multi-LLM is stopped.
- Ensures that corrupt records do not stall the log stream or panic the coordinator.

### 6.5 Terminal RAII Teardown (`TerminalGuard`)
- `struct TerminalGuard` in `src/main.rs` implements `Drop`:
  - Disables raw mode (`disable_raw_mode()`).
  - Leaves alternate screen (`LeaveAlternateScreen`).
  - Disables bracketed paste and mouse capture (`DisableMouseCapture`).
  - Restores cursor visibility (`crossterm::cursor::Show`).
- Guarantees complete terminal restoration on clean shutdown, `?` error return, or unexpected panic.

---

## 7. Verification & Test Suite

The Multi-LLM test suite enforces complete verification across all subsystems:

```bash
cargo test
```

### Verified Test Suite (100 Tests Passing)
- **Unit Tests (`src/lib.rs` - 64 tests)**:
  - `app::tests::test_mouse_selection_clamping_to_pane`
  - `app::tests::test_base64_encode_and_screen_text_extraction`
  - `app::tests::test_pageup_pagedown_scrollback`
  - `app::tests::test_cycle_responder_tab`
  - `app::tests::test_app_routing_and_fallback`
  - `chat::tests::test_load_new_messages_from_offset` (incremental seek & EOF protection)
  - `chat::tests::test_chat_message_roundtrip`
  - `chat::tests::test_extract_mentions` & `test_strip_at_mentions`
  - `config::tests::test_parse_prefix_key` & `test_config_collision_reassignment`
  - `ui::tests::test_calculate_layout` & `test_sender_palette_coverage`
  - `ui::tests::test_render_coordinator_multiline_composer`
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
  - `pty::tests::test_vt100_patch_cursor_save_restore` / `test_vt100_patch_ind_nel` / `test_vt100_patch_ind_scrolls_at_bottom` (vendored vt100 patch regression)
- **Reviewer Audit Regression Tests (`tests/reviewer_audit_regressions.rs` - 4 tests)**: permanent coverage of the nudge-echo, early-scrape, watchdog-clearing and UTF-8 truncation defects.
- **Integration Tests (`tests/integration_tests.rs` - 32 tests)**:
  - `test_task_dispatch_mandatory_protocol_suffix`
  - `test_watchdog_fallback_and_idempotent_uuid5`
  - `test_watchdog_nudge_echo_progression`
  - `test_salvage_pass_fused_records`
  - `test_chat_message_json_protocol`
  - `test_config_loading_and_defaults`
  - `test_app_init_stops_and_errors_on_invalid_command`
  - `test_client_pane_with_command_line_arguments`
  - `test_layout_symmetry` (verifies 1 through 6 pane symmetrical partitioning)
  - `test_atomic_composite_prompt_lazy_injection`
  - `test_role_instructions_isolated_routing`
  - `test_forced_responder_assignment_and_earliest_default`
  - `test_multiline_multiple_peer_mentions_routing`
  - `test_tab_autocomplete_available_agents`
  - `test_startup_instructions_playback_on_app_init`
  - `test_update_workflow_dry_run_and_check`
  - `test_update_atomic_replacement_integrity`

---

## 8. Getting Started

### Installation

#### Automated Install (Linux & macOS)
Install the pre-built `mllm` executable directly using the installation script:

```bash
# Install latest release into /usr/local/bin or ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/yeusin/mllm/main/install.sh | bash
```

Or run directly from a cloned repository:
```bash
./install.sh
```

Custom installation directory or specific version:
```bash
# Install to custom directory
curl -fsSL https://raw.githubusercontent.com/yeusin/mllm/main/install.sh | bash -s -- --bin-dir ~/.local/bin

# Install specific release tag
curl -fsSL https://raw.githubusercontent.com/yeusin/mllm/main/install.sh | bash -s -- --version v0.1.0
```

### Upgrading & In-Place Updates (`mllm update`)

Multi-LLM includes a native self-update mechanism via the `mllm update` command. It automatically resolves your host architecture and operating system, queries GitHub releases, validates SHA-256 asset checksums, and performs an atomic in-place binary replacement to prevent Unix `ETXTBSY` (Text file busy) execution locks.

```bash
# Check for updates without applying changes
mllm update --check

# Upgrade to the latest published release
mllm update

# Pin or switch to a specific version or release tag
mllm update --version v0.1.0

# Simulate the upgrade sequence without modifying files
mllm update --dry-run

# Force reinstallation of the same version
mllm update --force

# Update a specific target executable path
mllm update --target ~/.local/bin/mllm
```

#### CLI Flags & Options
| Flag | Short | Description |
| :--- | :--- | :--- |
| `--check` | `-c` | Queries the latest GitHub release and reports status without writing changes. |
| `--version <VER>` | `-v` | Target version or tag to install (e.g. `v0.1.0`). Defaults to `latest`. |
| `--force` | `-f` | Forces re-download and reinstallation even if already on the latest version. |
| `--dry-run` | | Simulates target detection, download URL, checksum URL, and replacement path without making any changes. |
| `--target <PATH>` | `-t` | Explicit executable path to replace. Defaults to the running executable (`std::env::current_exe`). |

#### Zero-Bloat Subprocess Architecture & Performance
Unlike traditional Rust update mechanisms that embed heavy crates (`self_update`, `reqwest`, `hyper`, `rustls`, or `ring`), `mllm update` delegates network transport and decompression to standard OS utilities (`curl`/`wget`, `tar`, and `sha256sum`/`shasum`/`openssl`):
- **Zero Added Dependencies**: Adds 0 transitive crates to the dependency graph, preserving rapid incremental release compile times (~5.7s).
- **Minimal Footprint**: The entire update subsystem occupies only **~16.5 KB** of compiled `.text` code (~0.3% of binary size), avoiding 1.8–2.8 MB of library bloat.
- **Ultra-Low Memory Overhead**: Runs with **<5 MB peak RSS** in `mllm`, delegating memory-intensive TLS buffering to ephemeral child processes.
- **Native OS Trust Store**: Seamlessly respects host system CA certificates and enterprise proxy configurations via `curl`/`wget` without requiring manual certificate bundles.

> [!NOTE]
> By avoiding heavy in-process TLS and decompression stacks, `mllm update` prevents OpenSSL shared library conflicts and keeps compilation times under 6 seconds.

#### Permission Remediation
- **System-Wide Installations (`/usr/local/bin` / `/usr/bin`)**:
  If `mllm` was installed in a system directory owned by root, run the update with elevated privileges:
  ```bash
  sudo mllm update
  ```
- **User-Local Installations (`~/.local/bin`)**:
  If you prefer unprivileged updates without `sudo`, install or target user space:
  ```bash
  mllm update --target ~/.local/bin/mllm
  ```
  Ensure `~/.local/bin` is in your `PATH` (`export PATH="$HOME/.local/bin:$PATH"`).

> [!TIP]
> To run self-updates unprivileged without requiring `sudo`, install `mllm` to `~/.local/bin` (`install.sh --bin-dir ~/.local/bin`) and ensure `~/.local/bin` is exported in your `PATH`.

#### Architecture & Integrity Safeguards
- **Platform Detection**: Automatically maps host OS (`linux`, `macos`), architecture (`x86_64`, `aarch64`), and target libc (`gnu` vs `musl`) to release assets.
- **Mandatory Fail-Closed SHA-256 Verification**: Fetches the official `.sha256` checksum asset (using `curl` with `wget` fallback). Verification is strictly enforced before decompression—if checksum retrieval fails or hashes diverge, execution halts immediately.
- **Hardened Multi-User Staging**: Unpacks archives within a private temporary directory restricted to `0700` permissions (`DirBuilderExt::mode(0o700)`) to mitigate multi-user `/tmp` snooping or tampering.
- **Payload & Archive Hardening**: Ensures extracted binary is a regular file (`is_file()`, rejecting symlinks and directories) and extracts archives using `--no-same-owner --no-same-permissions` to eliminate privilege escalation hazards under `sudo`.
- **Atomic In-Place Replacement & Fsync Durability**: Stages the binary into `.mllm-update-<uuid>.tmp` within the same target directory, applies `0755` permissions, flushes disk buffers via `sync_all()` (`fsync`) to prevent power-loss truncation, and executes an atomic POSIX `rename(2)` to overwrite the target executable cleanly without `ETXTBSY` lockouts.
- **Semantic Versioning & Intentional Downgrades**: Rejects invalid version strings or directory targets early. Downgrading to an older version (e.g. rollback to `v0.1.0`) is supported as an intentional resiliency feature: `mllm update` prints an explicit warning notice and proceeds unblocked:
  ```text
  Notice: Target version v0.1.0 is older than current version v0.2.0 (downgrade).
  ```

#### Trust Model & Supply-Chain Attestation (v0.2.1+)
- **Transport Integrity vs Compromise**: Built-in SHA-256 checksum verification guards against network transmission errors and corrupted downloads. Checksums are hosted alongside release archives and do not guard against upstream repository or release-pipeline compromise. Release v0.2.0 and earlier do not contain embedded cryptographic signatures.
- **Cryptographic SLSA Build Provenance**: Starting with release **v0.2.1**, Multi-LLM introduces Sigstore-backed GitHub Artifact Attestations (`actions/attest-build-provenance`).
  - **Non-Retroactivity Boundary**: Cryptographic SLSA attestations apply starting with **v0.2.1**; v0.2.0 and prior releases rely strictly on published SHA-256 transport checksums.
  - **Out-of-Band Verification (`gh >= 2.43`)**:
    ```bash
    gh attestation verify mllm-<target>.tar.gz \
      --repo yeusin/mllm \
      --signer-workflow yeusin/mllm/.github/workflows/release.yml
    ```
  - **Zero-Bloat Footprint Boundary**: Delegating attestation verification out-of-band to `gh` preserves `mllm`'s 0-dependency runtime footprint (<17 KB `.text`, <5 MB RSS) without embedding heavy Sigstore or OpenSSL dependencies.

For release history, see the [Release Notes](RELEASE_NOTES.md).

### Prerequisites (for building from source)
- **Rust Toolchain**: 1.85+ (Edition 2024 support)
- **Agent CLI**: [`agy`](https://github.com/google-deepmind/antigravity) (or any interactive CLI tool like `bash`, `python`, etc.)

### Build & Run from Source
```bash
# Clone repository
git clone git@github.com:yeusin/mllm.git
cd mllm

# Build optimized binary
cargo build --release

# Run with centralized configuration (auto-scaffolded at $HOME/.config/mllm/mllm.yaml on first launch)
./target/release/mllm
```

---

## 9. Troubleshooting

- **Enterprise proxies / SSL inspection**: If your environment blocks external connections or inspects SSL traffic, ensure `curl` or `wget` is configured with the appropriate certificates (e.g. in `/etc/ssl/certs`).
- **Configuration path**: Multi-LLM loads `$HOME/.config/mllm/mllm.yaml` by default, or an explicit path passed via `--config <path>` (`-c <path>`).
- **Per-directory logs**: Conversation logs are partitioned per working directory device and inode at `$HOME/.config/mllm/history/{dev}_{ino}/chat.jsonl`.
- **Rich formatting not rendering in an existing config**: Ensure `general_instructions` item 7 in your `mllm.yaml` includes:
  ```yaml
  7. Communication Standards: Be concise, clear, and actionable. Rich terminal formatting (inline & block Markdown, code blocks, ANSI colors, and emojis) is natively supported in the Center Coordinator chat.
  ```

---

## 10. License

Multi-LLM is open-source software licensed under the MIT License.
