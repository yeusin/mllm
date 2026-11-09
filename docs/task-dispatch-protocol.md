# Task Dispatch & Chat Logging Protocol v1.3

> **Status:** Final (v1.3). v1.3 adds the nudge-echo settle window (§1, §4), strict standalone prompt matching on the early path (§5), character-boundary truncation (§5) and current-ref-scoped task clearing (§7). Architect-approved; Reviewer audit findings R1-R9 are mandatory requirements. v1.1 added the dual-path prompt-match rule (§5) and cursor note (§4); v1.2 pair-scopes the §5 abort check and cap, defines `superseded_by` delivery, and adds sender normalization and §6 caveats.
> Not yet implemented. Describes target behavior for the coordinator (`src/app.rs`, `src/pty.rs`, `src/chat.rs`).
> The key words MUST, MUST NOT and MAY are used in their RFC 2119 sense.

## 1. Definitions

- **Turn**: one task dispatched to a client, identified by the task message `id` (`task_ref`).
- **Definition of Done**: a turn is complete only when a valid log entry with `reply_to == task_ref` and `sender == <client name>` exists in the shared log. Terminal output alone is ignored.
- **Activity**: either (a) a visible screen-content change (content-hash of the client's virtual terminal) or (b) growth of the chat log file. A silent shell append produces no PTY output, so (b) is required to avoid nudging agents that already logged.
- **Idle**: no Activity. **T_idle** is the time since the last Activity; any Activity resets it to 0 and `nudge_count` to 0.
- **Nudge-echo settle window**: after the coordinator sends a nudge, screen changes inside a short settle window (3s in the reference implementation) are caused by the echo of the nudge itself. They re-baseline `last_content_hash` and `hash_stable_since` but are NOT Activity: they MUST NOT reset `T_idle` or `nudge_count`. Without this, each nudge would restart the sequence and Nudge 2 and the scrape would never fire.

## 2. Task Dispatch Format

Every dispatched prompt, not only the first, is:

```text
Task from {sender} ({role}): {content} [Ref: {id}]

[MANDATORY PROTOCOL]: Your turn is NOT complete until you append a valid single-line JSON object to {chat_file} with "reply_to": "{id}", "sender": "{client_name}", "role": "{client_role}". Terminal output alone is ignored by the coordinator.
```

The suffix comes last (recency) and pre-fills `reply_to`, file path, sender and role. Once `mllm-reply` exists, the suffix should name it as the preferred command.

## 3. Log Entry Schema

One JSON object per line, UTF-8, newline-terminated.

| Field | Type | Required | Notes |
| :-- | :-- | :-- | :-- |
| `id` | string (UUID) | yes | unique |
| `reply_to` | string \| null | yes | question `id` |
| `sender` | string | yes | |
| `role` | string | yes | |
| `message` | string | yes | may start with `@Peer` mentions |
| `timestamp` | integer | yes | Unix epoch seconds |
| `fallback` | boolean | no | `true` only on coordinator-written fallback entries; omitted otherwise |
| `superseded_by` | string (UUID) | no | In-memory only (see below); never rewritten on disk |

Reference Rust type for `fallback`: `Option<bool>` with `skip_serializing_if = "Option::is_none"`. Existing parsers remain compatible.

**Fallback IDs are deterministic:** `uuid5(NAMESPACE_OID, "{task_ref}:{agent}:fallback")`. A double-fired fallback yields the same ID and is dropped by `known_message_ids` dedup.

**`superseded_by` delivery:** the log is append-only and is never rewritten in place (retro-edits shift bytes under concurrent readers). When a real reply with the same `reply_to` and sender arrives after a fallback, the coordinator sets `superseded_by` on the corresponding in-memory `chat_history` entry and the UI collapses it. The field is not persisted to the file.

## 4. Watchdog State Machine

State lives in `AppState` with its own timers. It must not live in `active_tasks` pruning (the 600s prune in `ui::render` must not own coordinator state), so turns longer than 600s stay tracked.

**Arming:** timers start at paste completion (after the typing queue drains), not at route time. The 15s post-dispatch grace is a floor (`task_elapsed >= 15s`) that overlaps the idle window and does not delay it.

| T_idle | Action |
| :-- | :-- |
| 25s | Nudge 1 (`nudge_count = 1`) |
| 40s | Nudge 2 (`nudge_count = 2`) |
| 55s | Scrape Fallback eligible, subject to §5 guards. May fire earlier on verified prompt restoration. |

Order of checks on every tick: (1) log growth / dedup (§7), (2) then nudge or scrape decision. Log growth always wins.

**Nudge delivery rules:**
- Nudges go through the same typing queue (`typing_tx`) as dispatch pastes. Never write to the PTY concurrently, since interleaved bytes corrupt the bracketed-paste payload.
- Do not gate on `is_busy` (true only for the ~50ms paste window).
- Max 2 nudges per task, on the fixed schedule above (no backoff).

Nudge 1 text (T_idle = 25s):

```text
[Coordinator Alert]: Task [{id}] response has not been logged. Please append your response as a JSON line to {chat_file} with "reply_to": "{id}" to complete your turn.
```

Nudge 2 text (T_idle = 40s, final notice):

```text
[Coordinator Final Alert]: Task [{id}] remains unlogged. If not appended to {chat_file} shortly, your terminal output will be automatically captured via scrape fallback.
```

Once `mllm-reply` is available, Nudge 1 instead says: `Please run: mllm-reply --reply-to {id} "..."`.

Optional, non-normative idle signals: cursor visibility (`!hide_cursor()`) and prompt-line heuristics may shorten the threshold (e.g. 12s). Hardware cursor visibility is strictly a heuristic and MUST NOT be a precondition: some TUIs draw a software block cursor and leave the hardware cursor hidden (`DECTCEM` disabled).

## 5. Scrape Fallback

**Preconditions (all required):**
1. `nudge_count == 2` and the T_idle = 55s point reached, or prompt restoration verified.
2. **Quiet window:** no screen mutation AND no log-size change for `T_quiet` (default 10s). On the T_idle = 55s path this holds by construction (55s > 10s); it is an independent check mainly for the early prompt-restoration path.
3. **Stable screen:** VT screen hash unchanged for 2 consecutive seconds, plus the prompt-match rule for the path taken:
   - **Early prompt-restoration path (T_idle < 55s):** the bottom prompt line MUST match the per-client prompt-state regex. The default match MUST be strict: a standalone prompt symbol (`>`, `$`, `❯`, `›`, as the whole line or a prefix followed by a space). It MUST NOT match lines that merely end in such a character or in `%` (progress bars, context-usage status lines, markdown quotes). The early path is not gated on `nudge_count`.
   - **Hard-timeout path (T_idle >= 55s after 2 nudges):** the regex match is OPTIONAL (best-effort). Bottom-row status bars and hints vary by CLI version and pane width and could otherwise prevent a match indefinitely. The mandatory gates are the 2s stable screen hash and the 10s quiet window (step 2). Byte silence alone is insufficient (TUIs repaint partially).
4. **Re-check under lock:** immediately before writing, and while holding the append lock, re-read the log from `last_chat_offset`. Abort if any entry has `reply_to == task_ref` AND `sender.eq_ignore_ascii_case(client.name)`. The check MUST be pair-scoped: on `@all` broadcasts a sibling agent's reply to the same `task_ref` MUST NOT abort this agent's scrape. The lock excludes only other lock-taking writers; agent shell appends are not blocked (§6), so a duplicate window remains and is made harmless by deterministic IDs, `fallback: true`, the cap and supersession.
5. **Cap:** at most 1 fallback per `(task_ref, sender)` pair. A per-`task_ref` cap would let the first fallback block other unresponsive agents on the same broadcast.

**Steps:**
1. Extract the response from the virtual screen. Cap the payload size (truncate on a UTF-8 character boundary, never mid-codepoint) and strip control-sequence residue.
2. Build an entry with `reply_to = task_ref`, the client's `sender`/`role`, `timestamp = now`, `"fallback": true`, deterministic `id` (§3).
3. Append through the locked path (§6) and mark the task complete.

**Non-routing:** the screen contains the coordinator's echoed prompt, which may include directive bullets (`- To @Reviewer: ...`). Any message with `fallback == true` MUST bypass mention extraction and peer routing in `route_message()` entirely, to prevent dispatch loops.

## 6. Append Protocol & Concurrency

- The coordinator is single-threaded (select loop). The real hazards are external agent writers and PTY-content timing.
- **Supported path:** `mllm-reply` calls `chat::append_message` (`O_APPEND` + exclusive `flock` + one `write_all` of the line plus newline). It must enforce flock, a single write, and the trailing newline. `mllm-reply` is the recommended path; the standalone subcommand/binary is a roadmap item and is not yet available. Until it ships, the per-turn suffix and nudges use the raw JSON-line wording.
- **Direct shell appends** (`>>`) are permitted but are not lock-protected: `flock` is advisory and arbitrates only coordinator and multi-instance writers. `O_APPEND` makes seek+write atomic on a regular file, but POSIX gives no size bound against interleaving (`PIPE_BUF` atomicity applies to pipes/FIFOs only). Shell users must emit the whole line in a single `printf` or heredoc write.
- **Partial-write recovery:** if an agent crashes mid-write, a no-newline tail can stall `load_new_messages_from_offset` and fuse with the next append. `load_new_messages_from_offset` MUST run a salvage pass: if a no-newline tail persists while the file grows, split at the `}{` boundary. Dead-lettered lines are replayed through salvage at startup.
- **Measured rationale (Reviewer):** single-write unlocked `>>` writers and the locked `append_message` coexisted without corruption (8 writers x 300 records plus 1 locked writer x 200: 2600/2600 valid). Writers that emitted each record as two writes corrupted 125 of 1000 lines (fused) even while the coordinator held `LOCK_EX`. An unlocked append completed in about 0.025 ms while `LOCK_EX` was held, confirming `flock` does not block `>>` writers.
- **Network filesystems:** a large single write may be split into several server-side WRITEs on NFS, allowing interleaving. On network mounts, agents MUST use `mllm-reply`.
- **Lock acquisition:** the append path SHOULD use `LOCK_NB` with a retry and timeout (about 2s) rather than a blocking `LOCK_EX`, so a stuck external lock holder cannot freeze the TUI event thread. `mllm-reply` MAY offer an optional fsync flag; a flush survives a process crash but not power loss.
- **Offset snapshot ordering:** `submit_user_message` must call `check_chat_log_updates()` immediately before appending its own message, then snapshot `last_chat_offset`. This prevents skipping an agent line that landed in the gap. The same ordering applies to coordinator-side fallback appends.

## 7. Deduplication Invariant (normative)

Before any nudge or fallback write, the coordinator filters on `(reply_to, sender)`. If the history already contains an entry with `reply_to == task_ref` and `sender == client.name`, no action is taken and the task is cleared. A change in log size or mtime resets the idle timer, and the coordinator MUST run `check_chat_log_updates()` to clear the task before any nudge or scrape decision (§1, §4). If a real reply arrives after a fallback, the fallback is tagged `superseded_by` and collapsed in the UI. Sender comparison trims and case-folds (`eq_ignore_ascii_case`). Matching uses only the currently tracked `task_ref` for that agent. A message from the agent clears the watchdog state only if `reply_to == task_ref` of the tracked task, not merely because the sender matches: a delayed reply to an older ref MUST NOT suppress nudges for the new turn. A message with `reply_to == task_ref` from an unrecognized sender SHOULD raise a Coordinator warning instead of silently mismatching.

## 8. Failure Modes

| Case | Behavior |
| :-- | :-- |
| Agent logs silently via shell | Log growth resets idle; no nudge, no scrape |
| Agent never logs, prompt restored | Scrape (after §5 guards) |
| Agent slow but screen active | No nudge; T_idle keeps resetting |
| Long silent command (>25s) | Nudge may fire; dedup prevents double entries once the agent writes |
| Agent append lands during scrape | §5 step 4 re-check aborts the scrape; otherwise `superseded_by` collapses it |
| Fallback double-fire | Same deterministic ID; dropped by dedup |
| Agent crash mid-write | Salvage pass (§6) |
| Turn longer than 600s | Watchdog state independent of UI prune |
| Process crash / API outage | Scrape if content exists; otherwise existing 600s UI timeout |

## 9. Suggested Tests

- Concurrent-append stress: N writers, all lines parse.
- Crash-mid-write recovery.
- Fallback double-fire idempotency.
- Real message during the grace/quiet window (scrape abort).
- Fallback entry containing directive bullets is not routed.
- Silent shell append suppresses nudge.
