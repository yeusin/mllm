# Multi-LLM (`mllm`) User Guide & Operational Manual

Welcome to the comprehensive user guide for **Multi-LLM (`mllm`)**, a high-performance terminal multiplexer and multi-agent orchestration TUI designed for collaborative autonomous agent workflows.

---

## Table of Contents
1. [Core Concepts & Architecture](#1-core-concepts--architecture)
   - [Coordinator vs Client Panes](#coordinator-vs-client-panes)
   - [Symmetrical Layout Engine](#symmetrical-layout-engine)
   - [Shared JSONL Event Bus](#shared-jsonl-event-bus)
2. [Installation & Lifecycle Management](#2-installation--lifecycle-management)
   - [Automated Quick Install](#automated-quick-install)
   - [Building from Source](#building-from-source)
   - [In-Place Upgrades with `mllm update`](#in-place-upgrades-with-mllm-update)
   - [Rollback Semantics & Downgrades](#rollback-semantics--downgrades)
   - [Security, Trust Model & Supply-Chain Attestation](#security-trust-model--supply-chain-attestation)
   - [Managing Permissions & Non-Root Upgrades](#managing-permissions--non-root-upgrades)
3. [Configuration Specification (`mllm.yaml`)](#3-configuration-specification-mllmyaml)
   - [Configuration Schema](#configuration-schema)
   - [Interactive Initialization (`mllm init`)](#interactive-initialization-mllm-init)
   - [Dynamic Instruction Template Variables](#dynamic-instruction-template-variables)
4. [Multi-Agent Coordination & Workflow](#4-multi-agent-coordination--workflow)
   - [Atomic Lazy Instruction Injection](#atomic-lazy-instruction-injection)
   - [Mention Syntax & Routing Rules](#mention-syntax--routing-rules)
   - [Coordinator Chat Stream & Multiline Composer](#coordinator-chat-stream--multiline-composer)
   - [Thinking Process & Real-Time Status Cards](#thinking-process--real-time-status-cards)
   - [Autonomous Chain Depth Circuit Breaker](#autonomous-chain-depth-circuit-breaker)
5. [Navigation, Keybindings & Terminal Emulation](#5-navigation-keybindings--terminal-emulation)
   - [Prefix Key Navigation](#prefix-key-navigation)
   - [Keybinding Reference Table](#keybinding-reference-table)
   - [Mouse Controls, Selection & Dual-Tier Clipboard](#mouse-controls-selection--dual-tier-clipboard)
   - [Process Supervision & Pane Respawns](#process-supervision--pane-respawns)
6. [Operational Best Practices & Troubleshooting](#6-operational-best-practices--troubleshooting)
   - [Resolving `ETXTBSY` Execution Locks](#resolving-etxtbsy-execution-locks)
   - [Checksum Verification & Security Auditing](#checksum-verification--security-auditing)
   - [Shared Log File Recovery & Dead-Letter Log](#shared-log-file-recovery--dead-letter-log)

---

## 1. Core Concepts & Architecture

Multi-LLM multiplexes multiple terminal-based AI agents (such as Google Antigravity CLI `agy`, Claude Code, Codex, or OpenCode) into an interactive, synchronized TUI environment without requiring `tmux` or `screen`.

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│                                 Multi-LLM TUI                                    │
├──────────────────────────┬───────────────────────────┬───────────────────────────┤
│  Pane 1: Architect       │  Pane 0: Coordinator Chat │  Pane 4: TechnicalWriter  │
│  [Role: Lead Architect]  │  [Role: Coordinator]      │  [Technical Writer]       │
│  (PTY: agy)              │  (Shared JSONL Stream)    │  (PTY: agy)               │
├──────────────────────────┤                           ├───────────────────────────┤
│  Pane 2: Reviewer        │  [#e1a2] User: Build task ├───────────────────────────┤
│  [Reviewer & Auditor]    │  [#f3b4] Architect: Plan  │  Pane 5: Designer         │
│  (PTY: agy)              │  [#a5c6] Reviewer: Audit  │  [Brand & UI/UX]          │
├──────────────────────────┤                           ├───────────────────────────┤
│  Pane 3: Researcher      │                           │  Pane 6: Marketer         │
│  [Role: Researcher]      │  [Composer: @Architect ]  │  [Growth & Campaign]      │
│  (PTY: agy)              │  [Tab: Cycle Responder]   │  (PTY: agy)               │
├──────────────────────────┴───────────────────────────┴───────────────────────────┤
│ [mllm] 0:Coordinator*  1:Architect  2:Reviewer  3:Researcher [Depth: 0/100] [C-b]│
└──────────────────────────────────────────────────────────────────────────────────┘
```

### Coordinator vs Client Panes
- **Center Coordinator (`Pane 0`)**: The human operator interface and autonomous conversation hub. It displays chronological, threaded chat messages, thinking process collapsible cards, active task timers, and a multiline message composer.
- **Client Panes (`Pane 1` to `Pane N`)**: Autonomous agent sessions running inside individual pseudo-terminal (PTY) pairs. Each client runs an interactive agent process that communicates with peers by reading and appending events to the shared event log.

### Symmetrical Layout Engine
Client panes are arranged symmetrically on the left and right sides of the central Coordinator pane:
- If 1 client is configured: Left [Pane 1], Center [Pane 0: Coordinator].
- If 2 clients are configured: Left [Pane 1], Center [Pane 0: Coordinator], Right [Pane 2].
- If 3 clients are configured: Left [Pane 1], Center [Pane 0: Coordinator], Right [Pane 2, Pane 3].
- If 4 clients are configured: Left [Pane 1, Pane 2], Center [Pane 0: Coordinator], Right [Pane 3, Pane 4].
- If 6 clients are configured: Left [Pane 1, Pane 2, Pane 3], Center [Pane 0: Coordinator], Right [Pane 4, Pane 5, Pane 6].

### Shared JSONL Event Bus
All communication between the user and agents occurs asynchronously via an append-only JSON Lines log file (default: `shared_chat.jsonl`). Multi-LLM monitors this file using non-blocking, $O(1)$ byte-offset polling with file locking to ensure safe concurrent access.

---

## 2. Installation & Lifecycle Management

### Automated Quick Install
Install the precompiled binary for your architecture and OS (Linux `x86_64`/`aarch64` and macOS `x86_64`/`Apple Silicon`/`Universal`):
```bash
# Install to default location (/usr/local/bin if root/sudo, or ~/.local/bin)
curl -fsSL https://raw.githubusercontent.com/yeusin/mllm/main/install.sh | bash

# Install to a custom directory
curl -fsSL https://raw.githubusercontent.com/yeusin/mllm/main/install.sh | bash -s -- --bin-dir ~/.local/bin

# Install a specific release tag
curl -fsSL https://raw.githubusercontent.com/yeusin/mllm/main/install.sh | bash -s -- --version v0.2.0
```

### Building from Source
Ensure you have the Rust toolchain (1.85+ with 2024 edition support) installed:
```bash
git clone https://github.com/yeusin/mllm.git
cd mllm
cargo build --release
sudo cp target/release/mllm /usr/local/bin/
```

### In-Place Upgrades with `mllm update`
Starting with **v0.2.0**, Multi-LLM features an autonomous self-updater. It enforces fail-closed SHA-256 transport integrity checksums, ensures atomic POSIX file replacement, and runs with zero external runtime dependencies.

```bash
# Check if an update is available without modifying files
mllm update --check

# Upgrade to the latest published release
mllm update

# Pin or downgrade to a specific release version (intentional rollback)
mllm update --version v0.1.0

# Simulate the upgrade sequence without altering your system
mllm update --dry-run

# Reinstall or force-refresh the current version
mllm update --force

# Update a specific executable path
mllm update --target ~/.local/bin/mllm
```

#### CLI Flags Reference (`mllm update`)
| Flag | Short | Description |
| :--- | :--- | :--- |
| `--check` | `-c` | Queries GitHub Releases, compares semantic versions, and exits 0 without writing files. |
| `--version <TAG>` | `-v` | Targets a specific release tag (e.g. `v0.2.0` or `0.2.0`). Defaults to `latest`. |
| `--force` | `-f` | Re-downloads and overwrites the executable even if already up to date. |
| `--dry-run` | | Resolves architecture, target URLs, and destination paths; prints diagnostics without writing. |
| `--target <PATH>` | `-t` | Explicit path to replace. Defaults to `std::env::current_exe()`. |

#### Rollback Semantics & Downgrades
Multi-LLM treats version downgrading as a deliberate, supported resiliency feature:
- If a target release tag is older than the currently running executable (e.g., executing `mllm update --version v0.1.0` while running v0.2.0), `mllm update` emits an explicit warning notice:
  ```text
  Notice: Target version v0.1.0 is older than current version v0.2.0 (downgrade)
  ```
- The operation is **unblocked** and completes normally, allowing operators to rollback quickly if an issue arises in a newer release.

### Security, Trust Model & Supply-Chain Attestation

#### Transport Integrity Model
The `mllm update` command verifies **transport integrity** before decompressing or replacing files:
- **Corrupted Download Protection**: The SHA-256 hash of the downloaded tarball is verified against the published `.sha256` asset. Missing, empty, or mismatched checksums cause an immediate, fail-closed abort.
- **Origin & Threat Scope**: Checksums are hosted alongside release archives on GitHub. They guard against network transmission errors, partial downloads, and disk corruption. They do **not** guard against upstream repository or release pipeline compromise, and releases in v0.2.0 and prior do not contain embedded cryptographic release signatures.

#### Out-of-Band Cryptographic SLSA Attestations (Starting v0.2.1)
Starting with release **v0.2.1**, Multi-LLM adopts cryptographic GitHub Artifact Attestations (`actions/attest-build-provenance`) backed by Sigstore/in-toto SLSA v1.0 specifications.

> [!IMPORTANT]
> **Non-Retroactivity Boundary**: Cryptographic SLSA attestations apply starting with **v0.2.1**. Release `v0.2.0` and earlier releases rely strictly on published SHA-256 transport checksums.

- **Prerequisites**: GitHub CLI [`gh`](https://cli.github.com/) version `2.43` or higher (`gh --version`).
- **Verification Workflow**: Verify the cryptographic provenance of release artifacts out-of-band:
  ```bash
  gh attestation verify mllm-<target>.tar.gz \
    --repo yeusin/mllm \
    --signer-workflow .github/workflows/release.yml
  ```
- **Zero-Bloat Architecture Boundary**: To preserve `mllm`'s ultra-lightweight footprint (<17 KB `.text` footprint, <5 MB RSS peak, 0 transitive dependencies, and sub-6-second release build times), cryptographic attestation validation is intentionally delegated out-of-band to `gh attestation verify` rather than embedding heavy cryptographic libraries (e.g. `ring`, `rustls`, `sigstore`) inside the `mllm update` binary.

### Managing Permissions & Non-Root Upgrades
- **System Installations (`/usr/local/bin`)**:  
  When `mllm` is installed in a root-owned directory, invoke updates with elevated privileges:
  ```bash
  sudo mllm update
  ```
- **Unprivileged User Installations (`~/.local/bin`)**:  
  To upgrade without requiring `sudo`, install `mllm` to `~/.local/bin`:
  ```bash
  # Ensure ~/.local/bin is in your PATH:
  export PATH="$HOME/.local/bin:$PATH"

  # Update runs entirely within user space
  mllm update
  ```

---

## 3. Configuration Specification (`mllm.yaml`)

### Configuration Schema
Multi-LLM reads configuration from `mllm.yaml` in the working directory (or from `--config <path>`).

```yaml
chat_file: "shared_chat.jsonl"
prefix_key: "ctrl+b"

# Universal coordination protocol injected into all agents
general_instructions: |
  Shared Log Coordination & Output Protocol:
  1. Shared Log File: All clients coordinate and store outputs by appending to {chat_file}.
  2. Output Storage in Shared Log: Write each output as a single valid JSON line followed by a newline:
     {"id": "<uuid>", "reply_to": "<question_uuid_or_null>", "sender": "<YourName>", "role": "<YourRole>", "message": "<Content with optional @Peer>", "timestamp": <unix_timestamp>}
  3. Unique UUID & Question Linking: Generate a unique UUID for "id" and reference the question's UUID with "reply_to".
  4. Unix Timestamp: The "timestamp" field must be an integer Unix timestamp in epoch seconds.
  5. Directing Tasks & Mention Format:
     - Header Mentions: Place target mentions at the start of a line or message (e.g. '@Reviewer: <task>').
     - Directive Bullet Points: Directives formatted as markdown list items (e.g. '- To @Reviewer: <task>') are automatically extracted and routed.
     - Broadcast: Mention '@all' at the start of a line to broadcast a task to all available peer agents.
     - Mid-Sentence Mentions: Mentions embedded in casual mid-sentence prose are ignored to prevent accidental routing loops.
  6. Mention Routing: The Center Coordinator monitors the shared log and automatically delivers @all or @mentioned tasks to target agents.
  7. Communication Standards: Be concise, clear, and actionable.
  8. Mandatory Result Output: Always output results and answers by appending to {chat_file}.

clients:
  - number: 1
    name: Architect
    role: Lead Architect
    command: agy
    instructions: |
      Lead system design, architecture specifications, and data modeling.
      Direct security and audit queries to @Reviewer in the shared log.
      Direct documentation and release notes to @TechnicalWriter in the shared log.

  - number: 2
    name: Reviewer
    role: Reviewer & Auditor
    command: agy
    instructions: |
      Audit code safety, correctness, test coverage, and documentation consistency.
      Direct architectural changes to @Architect in the shared log.

  - number: 3
    name: TechnicalWriter
    role: Technical Writer
    command: agy
    instructions: |
      Author technical documentation, user guides, API specs, and release notes.
      Direct design clarification requests to @Architect in the shared log.
      Direct accuracy verification queries to @Reviewer in the shared log.
```

### Interactive Initialization (`mllm init`)
Run `mllm init` to launch an interactive wizard that configures pane counts, role assignments, and client binaries (`agy`, `claude`, `codex`, `opencode`, or custom shells):
```bash
mllm init
```

### Dynamic Instruction Template Variables
Multi-LLM v0.2.0 introduces runtime template interpolation for system instructions. You can use standard variable placeholders in both `general_instructions` and client `instructions`:

| Variable | Aliases | Description | Example Replacement |
| :--- | :--- | :--- | :--- |
| `{chat_file}` | `{chatfile}` | Path to the shared JSONL event log | `shared_chat.jsonl` |
| `{prefix_key}` | `{prefixkey}` | Configured tmux-style prefix key | `ctrl+b` |
| `{name}` | `{client_name}` | Name assigned to the client agent | `Architect` |
| `{role}` | `{client_role}` | Role description of the client agent | `Lead Architect` |
| `{command}` | `{client_command}` | Command executable for the client | `agy` |
| `{number}` | `{client_number}` | Assigned client pane index | `1` |

> [!NOTE]
> - Spaces inside the variable braces (e.g. `{ chat_file }`) are permitted and normalized automatically.
> - Raw JSON notation such as `{"id": "<uuid>"}` or `{"name": "test"}` is preserved and never misidentified as a template variable.

---

## 4. Multi-Agent Coordination & Workflow

### Atomic Lazy Instruction Injection
In v0.1.0, startup instructions were broadcasted to `shared_chat.jsonl` on launch, cluttering chat logs and unnecessarily consuming token context before work began.

In **v0.2.0**, Multi-LLM uses **Atomic Lazy Instruction Injection**:
1. **Pristine Event Logs**: No synthetic startup instructions are ever appended to `shared_chat.jsonl`.
2. **Deferred Composite Prompting**: An agent remains in an uninitialized state until the Center Coordinator routes its **very first task prompt**.
3. **Atomic Assembly**: When the first task arrives, Multi-LLM atomically prepends the interpolated `[General Instructions & Protocol]` and `[Role Instructions]` to the incoming task:
   ```text
   [General Instructions & Protocol]
   <Interpolated General Instructions>

   [Role Instructions]
   Name: Architect
   Role: Lead Architect
   <Interpolated Role Specifics>

   Task from User (Human Operator): Design system architecture [Ref: 8e5426b]
   ```
4. **Subsequent Optimization**: Once initialized, all subsequent task prompts for that agent omit the instructions header and deliver only the direct task content:
   ```text
   Task from Reviewer (Reviewer & Auditor): Please audit section 2 [Ref: 91c382a]
   ```
5. **Warm Start Compatibility**: If an existing `shared_chat.jsonl` log is resumed, the same lazy injection guarantees that agents receive their role definitions on their first active task without replaying old historical instructions.

### Mention Syntax & Routing Rules
Multi-LLM choreographs multi-agent communication by inspecting the shared event log:

- **Header Mentions**: Mentions at the start of a message route directly to that agent:
  ```text
  @Architect: Please design the database schema.
  @Reviewer @Researcher: Please evaluate the proposed cryptographic scheme.
  ```
- **Directive Bullet Items**: Directed items in markdown lists are automatically extracted:
  ```text
  - To @Reviewer: Verify the error handling in lib.rs.
  - @TechnicalWriter: Update the user guide with new CLI flags.
  ```
- **Broadcasting (`@all`)**:
  ```text
  @all: Multi-platform release v0.2.0 is live. Stand by for team status updates.
  ```
- **Mid-Sentence Filtering**: Mentions embedded inside prose (e.g. `we met with @Architect earlier`) are ignored, preventing unintentional routing feedback loops.
- **Auto Fallback**: If a user enters a message without any `@mention`, the Coordinator routes the message to the earliest available agent (`@Architect` by default).

### Coordinator Chat Stream & Multiline Composer
- **Stream View**: Displays chronological events with local timestamps (`YYYY-MM-DD HH:MM:SS`), sender role tags, and unique short UUIDs (`[#e1a2]`).
- **Multiline Composer**:
  - `Enter`: Insert a newline into your message draft.
  - `Ctrl+S` (or `F5`): Dispatch message to the shared event stream.
  - `Tab`: Auto-complete `@agent` mentions or cycle the active target responder.
  - `Esc`: Clear composer buffer or unfocus.

### Thinking Process & Real-Time Status Cards
Multi-LLM detects LLM thinking indicators (e.g. `<thought>` tags or `Thinking Process:` headers) in agent PTY streams:
- Renders an animated thinking card in the Coordinator status banner.
- Displays elapsed execution time (`Thinking (14.2s)...`).
- Automatically transitions to completed status as soon as output begins streaming.

### Autonomous Chain Depth Circuit Breaker
To prevent runaway autonomous loops or infinite conversation recursion between agents, Multi-LLM enforces an autonomous chain depth limiter (`max_chain_depth = 100`):
- Each autonomous peer-to-peer response increments the active chain counter.
- When `active_chain_depth >= 100`, autonomous dispatch pauses with a warning:
  `"Chain depth limit reached (100). Pausing autonomous agent turns."`
- The human operator can resume execution at any time by sending a message from the composer, which automatically resets `active_chain_depth = 0`.

---

## 5. Navigation, Keybindings & Terminal Emulation

### Prefix Key Navigation
Like `tmux`, Multi-LLM commands are triggered by pressing a configurable prefix key (default: `Ctrl+b`), followed by a command key.

When the prefix key is armed, the status bar displays an illuminated `[PREFIX]` indicator.

### Keybinding Reference Table

| Keybinding | Action | Description |
| :--- | :--- | :--- |
| `Ctrl+b` then `0` | Select Coordinator | Switch focus to the central Coordinator chat pane. |
| `Ctrl+b` then `1`–`9` | Select Client Pane | Switch focus directly to Client Pane 1 through 9. |
| `Ctrl+b` then `o` | Cycle Next Pane | Focus the next pane in sequence. |
| `Ctrl+b` then `O` | Cycle Prev Pane | Focus the previous pane in sequence. |
| `Ctrl+b` then `z` | Zoom Pane | Toggle full-screen zoom for the currently focused pane. |
| `Ctrl+b` then `[` | Scrollback Mode | Enter pane scrollback navigation. |
| `Ctrl+b` then `k` | Kill Process | Terminate the process running in the current pane. |
| `Ctrl+b` then `r` | Respawn Process | Restart the client process in the current pane. |
| `Ctrl+b` then `d` | Detach / Quit | Cleanly shut down Multi-LLM and restore terminal mode. |
| `PageUp` / `PageDown` | Scroll History | Page up or down in the focused pane's scrollback buffer. |

### Mouse Controls, Selection & Dual-Tier Clipboard
- **Click to Focus**: Left-clicking anywhere inside a pane activates and focuses it.
- **Drag Selection**: Click and drag to select text across any client or coordinator pane. Selection is clamped strictly to the active pane boundary.
- **Dual-Tier Clipboard Export**:
  - **Tier 1 (OSC 52)**: High-speed native terminal clipboard export supported across local terminals and SSH sessions.
  - **Tier 2 (OS System Fallback)**: Automatically falls back to native system clipboard tools (`wl-copy`, `xclip`, `pbcopy`) if OSC 52 is unavailable.
- **Quick Copy**: Press `Ctrl+c` or right-click to copy highlighted text.

### Process Supervision & Pane Respawns
Multi-LLM actively monitors the PTY file descriptors of all client processes:
- If an agent exits or crashes, the pane border indicates `[EXITED]` with its exit code.
- Press `Ctrl+b` followed by `r` to respawn the failed process in place without restarting Multi-LLM.

---

## 6. Operational Best Practices & Troubleshooting

### Resolving `ETXTBSY` Execution Locks
On Unix platforms, attempting to overwrite a running binary directly triggers `ETXTBSY` (Text file busy).  
`mllm update` eliminates this issue through atomic staging:
1. It writes the new binary to `.mllm-update-<uuid>.tmp` in the same directory.
2. It sets permissions to `0755` and calls `sync_all()` (`fsync`).
3. It performs a POSIX atomic `rename(2)` over the destination. The running binary's inode is unlinked and freed as soon as the current process terminates.

### Checksum Verification & Security Auditing
`mllm update` enforces fail-closed SHA-256 verification:
- Before any archive is decompressed, the SHA-256 checksum is downloaded and compared against the file hash.
- Archives are extracted with `--no-same-owner --no-same-permissions` inside a private temporary directory (`0700` permissions).
- **Trust Scope**: Published SHA-256 checksums guard against transport corruption and incomplete network downloads. They do not guard against upstream pipeline compromise. For high-assurance environments requiring cryptographic SLSA build provenance, verify releases out-of-band starting with v0.2.1 using `gh attestation verify` (`gh >= 2.43`).
- If your enterprise environment blocks external connections or utilizes an SSL inspection proxy, ensure `curl` or `wget` is configured with appropriate certificates in `/etc/ssl/certs`.

### Shared Log File Recovery & Dead-Letter Log
- If a malformed JSON line is appended to `shared_chat.jsonl` (e.g. from an experimental manual tool), Multi-LLM skips the corrupted record, logs the malformed content to `.mllm_dead_letter.jsonl`, and continues processing subsequent lines without interruption.
- The shared log can be safely truncated or rotated when Multi-LLM is stopped.
