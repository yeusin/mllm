# Multi-LLM (`mllm`) Release Notes

---

## Release v0.2.0 (November 2026)

**Theme**: Autonomous Self-Update, Atomic Lazy Injection & Instruction Templating  
**Release Tag**: [`v0.2.0`](https://github.com/yeusin/mllm/releases/tag/v0.2.0)  
**Commit**: `8e5426b`  

Multi-LLM v0.2.0 is a major milestone release focusing on seamless application lifecycle management, multi-agent instruction ergonomics, and token context efficiency. It introduces native in-place self-updates via `mllm update`, dynamic configuration variable interpolation, and atomic lazy instruction injection that completely eliminates startup chat file pollution.

---

### Highlights & Key Features

#### 1. Native Self-Update Command (`mllm update`)
Multi-LLM now provides a built-in, unassisted upgrade mechanism that automatically discovers the host target architecture, queries GitHub Releases for the latest or specified version, validates cryptographic integrity via SHA-256 checksums, and safely replaces the running executable in place.

- **Zero-Bloat Subprocess Architecture**: Avoids adding heavy runtime networking or cryptographic dependencies (`reqwest`, `hyper`, `rustls`, `ring`, or `self_update`). Subprocess orchestration via native OS utilities (`curl`/`wget`, `tar`, `sha256sum`/`shasum`/`openssl`) preserves lightweight binary footprint (~16.5 KB compiled text footprint, ~0.3% of total binary size) and maintains sub-6-second release compile times.
- **Atomic In-Place Replacement**: Eliminates Unix `ETXTBSY` (Text file busy) execution lockups by staging the downloaded executable into `.mllm-update-<uuid>.tmp` within the same target directory, applying `0755` executable permissions, executing `sync_all()` (`fsync`) for disk durability, and performing an atomic POSIX `rename(2)` over the target path.
- **Fail-Closed SHA-256 Checksum Verification**: Mandates cryptographic hash verification before extracting any archive. Automatically checks primary and fallback checksum URL naming conventions (`mllm-<target>.sha256` and `mllm-<target>.tar.gz.sha256`). Mismatched or unretrievable checksums halt immediately before altering files.
- **Hardened Multi-User Staging**: Extracts archives into a private temporary directory restricted to `0700` (`DirBuilderExt::mode(0o700)`) and invokes `tar` with `--no-same-owner --no-same-permissions` to eliminate privilege escalation hazards during `sudo` upgrades. Extracted payloads are validated as non-symlink regular files.
- **Flexible CLI Controls**:
  - `mllm update --check` (`-c`): Inspect available updates against the local version without downloading or writing files.
  - `mllm update --version <VER>` (`-v`): Pin to an exact release tag (e.g. `v0.1.0`) or switch versions on demand.
  - `mllm update --force` (`-f`): Force reinstallation or binary refresh of the current version.
  - `mllm update --dry-run`: Preview resolved platform target, download URLs, checksum URLs, and atomic replacement targets without filesystem side-effects.
  - `mllm update --target <PATH>` (`-t`): Explicitly target user-local installations (e.g. `~/.local/bin/mllm`) or system paths (`/usr/local/bin/mllm`).
- **Downgrade Notices & Semantic Versioning**: Validates version strings against semantic versioning schemas and explicitly issues a downgrade confirmation notice when targeting older releases.

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
| **Checksum Enforcement** | SHA-256 hash calculation occurs before decompression; missing or diverging checksums halt execution with non-zero exit codes. |
| **Temporary Staging Sandbox** | Temporary directories use `0700` mode, preventing multi-user `/tmp` race conditions or symbolic link hijacks. |
| **Extraction Hardening** | Invocations of `tar` enforce `--no-same-owner --no-same-permissions` to eliminate privilege escalation hazards when running under `sudo`. |
| **Subprocess Efficiency** | Memory consumption during updates remains under **5 MB peak RSS**, eliminating OpenSSL runtime linking conflicts. |

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

## Release v0.1.0 (November 2026)

**Initial Public Release**
- High-performance, tmux-free terminal multiplexer built in Rust with `portable-pty` and `vt100`.
- Symmetrical multi-agent layout arranging client PTYs evenly around the center Coordinator pane.
- Atomic append-only JSONL shared event bus (`shared_chat.jsonl`) with `O(1)` byte-offset polling.
- Real-time LLM status indicators, bracketed paste prompt injection, and autonomous `@mention` routing with a 100-turn safety circuit breaker.
- Cross-platform installer script (`install.sh`) supporting Linux and macOS (x86_64, aarch64, and Apple Universal binary).
