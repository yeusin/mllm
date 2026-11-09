//! Reviewer audit regression tests (Protocol v1.1 implementation review).
//!
//! These tests FAIL against commit state `8445397c` BY DESIGN: each one
//! reproduces a confirmed defect reported in the shared-log audit
//! (refs: d2bf6c3a, 5dd4071e). They define the fix targets:
//!
//! 1. `test_regression_nudge_echo_must_not_reset_escalation`
//!    - Nudge-induced screen echo must not reset `nudge_count`, otherwise
//!      Nudge 2 and the 55s timeout scrape are unreachable (TW d2bf6c3a #1,
//!      Researcher 5dd4071e #1).
//! 2. `test_regression_early_scrape_requires_nudge_and_strict_prompt`
//!    - Early prompt-restoration scrape must not fire on generic line endings
//!      like `42%` with zero nudges (TW d2bf6c3a #2, Researcher 5dd4071e #2).
//! 3. `test_regression_scrape_truncation_is_utf8_safe`
//!    - `clean_scraped_terminal_text` must not panic on multi-byte UTF-8
//!      content straddling the 32768-byte cap (Researcher 5dd4071e #3).
//! 4. `test_regression_watchdog_survives_non_reply_sender_message`
//!    - `check_chat_log_updates` must only clear a watchdog task on a reply
//!      matching the tracked `task_ref`, not on ANY message from the sender
//!      (TW d2bf6c3a #3, spec §7).

use mllm::config::{AppConfig, ClientConfig};
use mllm::app::AppState;
use std::time::{Duration, Instant};

fn make_app(client_name: &str, chat_file: &std::path::Path) -> AppState {
    let cfg = AppConfig {
        mandatory_protocol: String::new(),
        prefix_key: "alt".to_string(),
        general_instructions: "Protocol instructions".to_string(),
        clients: vec![ClientConfig {
            number: 1,
            name: client_name.to_string(),
            role: "Lead Architect".to_string(),
            command: "sh -c 'sleep 60'".to_string(),
            instructions: "Lead architecture".to_string(),
            extra_args: vec![],
        }],
    };
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    AppState::new(cfg, Some(chat_file), event_tx).unwrap()
}

fn age_task(app: &mut AppState, dispatched_ago_s: u64, idle_s: u64, stable_s: u64) {
    let now = Instant::now();
    let wd = app.watchdog_tasks.get_mut("architect").unwrap();
    wd.dispatched_at = now - Duration::from_secs(dispatched_ago_s);
    wd.last_activity_time = now - Duration::from_secs(idle_s);
    wd.hash_stable_since = now - Duration::from_secs(stable_s);
    // Baseline the hash so screen-mutation logic does not fire spuriously.
    wd.last_content_hash = app.clients[0].current_screen_hash();
}

fn echo_on_screen(app: &mut AppState, text: &str) {
    // Deterministically reproduce what a real agent TUI does with a nudge:
    // renders the pasted text, mutating the visible screen (and its hash).
    if let Ok(mut parser) = app.clients[0].vt_parser.lock() {
        parser.process(text.as_bytes());
    }
}

/// TW d2bf6c3a #1 / Researcher 5dd4071e #1 (CRITICAL).
/// Spec §4: nudge-induced echo must not reset the escalation ladder.
#[tokio::test]
async fn test_regression_nudge_echo_must_not_reset_escalation() {
    let tmp = std::env::temp_dir().join(format!("rev_echo_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let chat_file = tmp.join("chat.jsonl");
    let mut app = make_app("Architect", &chat_file);

    let task = chat_message_for_test("@Architect Echo regression".to_string());
    app.route_message(&task);
    assert!(app.watchdog_tasks.contains_key("architect"));

    // T+30s dispatch, 26s idle -> Nudge 1 must fire.
    age_task(&mut app, 30, 26, 26);
    app.check_watchdog();
    assert_eq!(
        app.watchdog_tasks.get("architect").unwrap().nudge_count, 1,
        "Nudge 1 must fire at 25s idle"
    );

    // The nudge paste is echoed/rendered on the agent screen.
    echo_on_screen(&mut app, "[Coordinator Alert]: echoed nudge text");

    // Per spec, a nudge-induced screen change must NOT reset nudge_count.
    app.check_watchdog();
    assert_eq!(
        app.watchdog_tasks.get("architect").unwrap().nudge_count, 1,
        "BUG: nudge echo reset nudge_count to 0; escalation ladder restarts forever"
    );

    // Continue the ladder: 41s idle -> Nudge 2, then 56s idle -> scrape.
    age_task(&mut app, 71, 41, 41);
    app.watchdog_tasks.get_mut("architect").unwrap().nudge_count = 1;
    app.check_watchdog();
    assert_eq!(
        app.watchdog_tasks.get("architect").unwrap().nudge_count, 2,
        "Nudge 2 must fire at 40s idle"
    );

    age_task(&mut app, 112, 56, 56);
    app.watchdog_tasks.get_mut("architect").unwrap().nudge_count = 2;
    app.check_watchdog();
    assert!(
        app.chat_history.iter().any(|m| m.fallback == Some(true)),
        "BUG: 55s timeout scrape never reached after nudge echo"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// TW d2bf6c3a #2 / Researcher 5dd4071e #2.
/// Spec §5: early restoration must require a nudge first and a strict
/// prompt match; a `42%` status line with 0 nudges must NOT scrape.
#[tokio::test]
async fn test_regression_early_scrape_requires_nudge_and_strict_prompt() {
    let tmp = std::env::temp_dir().join(format!("rev_early_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let chat_file = tmp.join("chat.jsonl");
    let mut app = make_app("Architect", &chat_file);

    let task = chat_message_for_test("@Architect Early scrape regression".to_string());
    app.route_message(&task);

    // Agent screen shows a typical status bar with a percentage.
    {
        let mut parser = app.clients[0].vt_parser.lock().unwrap();
        parser.process(b"Working...\r\nContext: 42%");
    }

    // 12s idle (>=10s quiet, <55s), screen stable 3s, ZERO nudges so far.
    age_task(&mut app, 20, 12, 3);
    app.check_watchdog();

    assert!(
        !app.chat_history.iter().any(|m| m.fallback == Some(true)),
        "BUG: early scrape fired at 12s idle on a '42%' status line with no nudge"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Researcher 5dd4071e #3.
/// `clean_scraped_terminal_text` slices at raw byte 32768; box-drawing and
/// spinner glyphs are multi-byte, so the boundary can split a codepoint (panic).
#[test]
fn test_regression_scrape_truncation_is_utf8_safe() {
    // U+2500 '─' is 3 bytes; 12000 * 3 = 36000 > 32768 and 32768 % 3 != 0,
    // so the old `&joined[..32768]` slice lands inside a codepoint.
    let raw = "─".repeat(12000);
    let out = mllm::app::clean_scraped_terminal_text(&raw);
    assert!(out.is_char_boundary(out.len()), "truncation must respect UTF-8 boundaries");
}

/// TW d2bf6c3a #3 (spec §7).
/// A message from the agent that does NOT reply to the tracked task_ref
/// (e.g. null reply_to) must not clear the watchdog task.
#[tokio::test]
async fn test_regression_watchdog_survives_non_reply_sender_message() {
    let tmp = std::env::temp_dir().join(format!("rev_anymsg_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let chat_file = tmp.join("chat.jsonl");
    let mut app = make_app("Architect", &chat_file);

    let task = chat_message_for_test("@Architect Non-reply clearing regression".to_string());
    app.route_message(&task);
    assert!(app.watchdog_tasks.contains_key("architect"));

    // Agent emits an unrelated message (null reply_to) directly to the log.
    let unrelated = mllm::chat::ChatMessage {
        id: "unrelated-1".to_string(),
        reply_to: None,
        sender: "Architect".to_string(),
        role: "Lead Architect".to_string(),
        message: "broadcast announcement".to_string(),
        timestamp: mllm::chat::current_timestamp(),
        fallback: None,
        superseded_by: None,
    };
    mllm::chat::append_message(&chat_file, &unrelated).unwrap();

    app.check_chat_log_updates();

    assert!(
        app.watchdog_tasks.contains_key("architect"),
        "BUG: watchdog task cleared by a non-reply message from the sender (spec §7 violation)"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// Local helper (test-only user message; mirrors ChatMessage::new_user).
fn chat_message_for_test(message: String) -> mllm::chat::ChatMessage {
    mllm::chat::ChatMessage::new_user(message)
}
