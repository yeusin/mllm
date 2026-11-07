use mllm::chat::{extract_mentions, format_local_timestamp, strip_at_mentions, ChatMessage};
use mllm::config::load_config;
use mllm::ui::calculate_layout;
use ratatui::layout::Rect;
use std::path::Path;

#[test]
fn test_message_local_timestamp_formatting() {
    let msg = ChatMessage::new_user("Test timestamp".to_string());
    let formatted = format_local_timestamp(msg.timestamp);
    assert_eq!(formatted.len(), 19);
    assert!(formatted.contains('-'));
    assert!(formatted.contains(':'));
}

#[test]
fn test_config_loading_and_defaults() {
    let config_path = if Path::new("mllm.yaml").exists() {
        Path::new("mllm.yaml")
    } else {
        Path::new("mllm.yaml.example")
    };
    let cfg = load_config(config_path).expect("Failed to load mllm.yaml or mllm.yaml.example");
    assert_eq!(cfg.chat_file, "shared_chat.jsonl");
    assert_eq!(cfg.prefix_key, "ctrl+b");
    assert!(!cfg.clients.is_empty());
    assert_eq!(cfg.clients[0].number, 1);
    assert_eq!(cfg.clients[0].name, "Architect");
    if cfg.clients.len() >= 2 {
        assert_eq!(cfg.clients[1].number, 2);
        assert_eq!(cfg.clients[1].name, "Reviewer");
    }
    if cfg.clients.len() >= 3 {
        assert_eq!(cfg.clients[2].number, 3);
        assert_eq!(cfg.clients[2].name, "Researcher");
    }
}

#[test]
fn test_mention_parsing_cases() {
    let input = "@Architect please coordinate with @Reviewer and @Researcher on @ALL";
    let mentions = extract_mentions(input);
    assert_eq!(mentions, vec!["Architect"]);

    // Mentions in the middle of the text are ignored
    let mid_input = "Please coordinate with @Reviewer and @Researcher on @ALL";
    assert_eq!(extract_mentions(mid_input), Vec::<String>::new());

    let stripped = strip_at_mentions(input);
    assert_eq!(
        stripped,
        "please coordinate with @Reviewer and @Researcher on @ALL"
    );

    // Ensure @all and role-specific @Architect instructions are stripped of mention tags
    let gen_instruction = "@all [General Instructions & Protocol]\nShared Log Coordination";
    assert_eq!(
        strip_at_mentions(gen_instruction),
        "[General Instructions & Protocol]\nShared Log Coordination"
    );

    let role_instruction = "@Architect [Role Instructions: Lead Architect]\nLead design decisions.";
    assert_eq!(
        strip_at_mentions(role_instruction),
        "[Role Instructions: Lead Architect]\nLead design decisions."
    );

    let role_instruction_with_peers = "@Reviewer [Role Instructions: Reviewer & Auditor]\nReview architecture, code quality, edge cases, vulnerability vectors, and concurrency safety.\nAudit reliability, error handling, memory safety, and maintenance overhead.\nDirect architectural redesign questions to @Architect in the shared log.\nDirect benchmark verification requests to @Researcher in the shared log.";
    assert_eq!(
        strip_at_mentions(role_instruction_with_peers),
        "[Role Instructions: Reviewer & Auditor]\nReview architecture, code quality, edge cases, vulnerability vectors, and concurrency safety.\nAudit reliability, error handling, memory safety, and maintenance overhead.\nDirect architectural redesign questions to @Architect in the shared log.\nDirect benchmark verification requests to @Researcher in the shared log."
    );
}

#[test]
fn test_layout_symmetry() {
    let full_area = Rect::new(0, 0, 192, 60);

    for count in 1..=6 {
        let (left, center, right, status) = calculate_layout(full_area, count);
        assert_eq!(status.height, 1);
        assert_eq!(status.y, 59);

        // Center coordinator pane is given 12/32 ratio (~37.5-38%)
        assert!(center.width >= left.width);
        assert!(center.width >= right.width);
    }
}

#[test]
fn test_chat_message_json_protocol() {
    let msg = ChatMessage::new_user("@Architect port mllm to Rust".to_string());
    let json_str = serde_json::to_string(&msg).expect("Serialize error");

    assert!(json_str.contains("\"sender\":\"User\""));
    assert!(json_str.contains("\"role\":\"Human Operator\""));
    assert!(json_str.contains("\"message\":\"@Architect port mllm to Rust\""));

    let deserialized: ChatMessage = serde_json::from_str(&json_str).expect("Deserialize error");
    assert_eq!(deserialized.id, msg.id);
    assert_eq!(deserialized.reply_to, None);
    assert_eq!(deserialized.sender, "User");
}

#[tokio::test]
async fn test_startup_instructions_playback_on_app_init() {
    use mllm::app::AppState;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_startup_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "Universal output protocol.".to_string(),
        clients: vec![ClientConfig {
            number: 1,
            name: "Architect".to_string(),
            role: "Lead Architect".to_string(),
            command: "cat".to_string(),
            instructions: "Design architecture.".to_string(),
            extra_args: vec![],
        }],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config.clone(), &tmp_dir, tx).expect("Failed to initialize AppState");

    // Give asynchronous typing playback task time to complete
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    // Under lazy instruction injection, no startup messages are pre-broadcast to shared_chat.
    // Client starts uninitialized until the first task prompt is routed.
    assert_eq!(app.chat_history.len(), 0);
    assert_eq!(app.initialized_clients.len(), 0);

    // Route first task prompt to Architect
    let user_msg = ChatMessage::new_user("@Architect Review initial architecture".to_string());
    app.route_message(&user_msg);
    assert!(app.initialized_clients.contains(&0));

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    {
        let parser = app.clients[0].vt_parser.lock().unwrap();
        let contents = parser.screen().contents();
        assert!(contents.contains("Universal output protocol."));
        assert!(contents.contains("Design architecture."));
    }

    // Check that client pane received simulated typing
    assert_eq!(app.clients.len(), 1);
    app.shutdown();

    // Restart app in the same directory (warm start).
    // Client starts uninitialized, and first task prompt lazily injects instructions.
    let (tx2, _rx2) = mpsc::unbounded_channel();
    let mut app2 = AppState::new(config, &tmp_dir, tx2).expect("Failed to restart AppState");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(app2.initialized_clients.len(), 0);
    let user_msg2 = ChatMessage::new_user("@Architect Please design the subsystem".to_string());
    app2.route_message(&user_msg2);
    assert!(app2.initialized_clients.contains(&0));

    app2.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_spawn_nonexistent_binary() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = mllm::pty::ClientPane::new(
        1,
        "TestAgent".to_string(),
        "TestRole".to_string(),
        "this_binary_does_not_exist_at_all_12345 --flag",
        &[],
        80,
        24,
        tx,
    );
    assert!(result.is_err());
    let err_msg = format!("{:#}", result.err().unwrap());
    assert!(err_msg.contains("Failed to spawn command"));
    assert!(err_msg.contains("this_binary_does_not_exist_at_all_12345"));
}

#[test]
fn test_client_pane_with_command_line_arguments() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = mllm::pty::ClientPane::new(
        1,
        "EchoAgent".to_string(),
        "EchoRole".to_string(),
        "echo --test-arg 'hello world'",
        &["extra".to_string()],
        80,
        24,
        tx,
    );
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_app_init_stops_and_errors_on_invalid_command() {
    use mllm::app::AppState;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_err_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![ClientConfig {
            number: 1,
            name: "InvalidAgent".to_string(),
            role: "Invalid".to_string(),
            command: "nonexistent_binary_xyz_404 --some-arg".to_string(),
            instructions: "".to_string(),
            extra_args: vec![],
        }],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let result = AppState::new(config, &tmp_dir, tx);
    assert!(result.is_err());
    let err = format!("{:#}", result.err().unwrap());
    assert!(err.contains("Failed to spawn command 'nonexistent_binary_xyz_404 --some-arg'"));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_forced_responder_assignment_and_earliest_default() {
    use mllm::app::AppState;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_responder_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config.clone(), &tmp_dir, tx).expect("Failed to initialize AppState");

    // 1. Earliest numbered agent should be Architect (number 1 < number 2)
    assert_eq!(app.default_responder(), Some("Architect"));

    // 2. Initial composer input should be pre-filled with the default responder
    assert_eq!(app.composer_input, "@Architect ");

    // 3. Submitting text WITHOUT mention forces assignment of earliest numbered agent
    app.submit_user_message("check concurrency model".to_string());
    let last_msg = app.chat_history.last().unwrap();
    assert_eq!(last_msg.message, "@Architect check concurrency model");

    // 4. Composer input is reset to default responder after submission
    assert_eq!(app.composer_input, "@Architect ");

    // 5. Submitting text WITH explicit mention preserves specified responder
    app.submit_user_message("@Reviewer review this code".to_string());
    let last_msg2 = app.chat_history.last().unwrap();
    assert_eq!(last_msg2.message, "@Reviewer review this code");

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}


#[tokio::test]
async fn test_multiline_multiple_peer_mentions_routing() {
    use mllm::app::AppState;
    use mllm::chat::ChatMessage;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_multi_peer_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 3,
                name: "Researcher".to_string(),
                role: "Researcher".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // Message from Architect with proposal header and multiple peer questions on subsequent lines
    let mut msg = ChatMessage::new_user(
        "[Proposal Title]\nAnalysis details...\n@Reviewer please check.\n@Researcher please benchmark.".to_string()
    );
    msg.sender = "Architect".to_string();
    msg.role = "Lead Architect".to_string();

    let targets = app.determine_target_clients(&msg);
    // Architect should not be targeted (sender excluded), but both Reviewer (idx 1) and Researcher (idx 2) must be targeted
    assert_eq!(targets, vec![1, 2]);

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_peer_consecutive_and_directive_mentions_routing() {
    use mllm::app::AppState;
    use mllm::chat::ChatMessage;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_consecutive_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 3,
                name: "Researcher".to_string(),
                role: "Researcher".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // Case 1: Multiple leading mentions on the same line with @User
    let mut msg1 = ChatMessage::new_user(
        "@User @Reviewer @Researcher Interactive `init` command implemented and verified".to_string(),
    );
    msg1.sender = "Architect".to_string();
    msg1.role = "Lead Architect".to_string();

    let targets1 = app.determine_target_clients(&msg1);
    // Architect is excluded (sender), User is filtered out (human), Reviewer (idx 1) and Researcher (idx 2) are targeted
    assert_eq!(targets1, vec![1, 2]);

    // Case 2: Directives listed in markdown bullet points
    let mut msg2 = ChatMessage::new_user(
        "Directives to Peers:\n  - To @Reviewer: Audit input sanitization\n  - To @Researcher: Benchmark setup".to_string(),
    );
    msg2.sender = "Architect".to_string();
    let targets2 = app.determine_target_clients(&msg2);
    assert_eq!(targets2, vec![1, 2]);

    // Case 3: User forwarding the same message
    let msg3 = ChatMessage::new_user(
        "@User @Reviewer @Researcher Interactive `init` command implemented and verified".to_string(),
    );
    let targets3 = app.determine_target_clients(&msg3);
    assert_eq!(targets3, vec![1, 2]);

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}


#[tokio::test]
async fn test_role_instructions_isolated_routing() {
    use mllm::app::AppState;
    use mllm::chat::ChatMessage;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_role_iso_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "Protocol instructions.".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "Lead architecture. Direct security to @Reviewer and announce to @all.".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer".to_string(),
                command: "cat".to_string(),
                instructions: "Review code. Direct questions to @Architect.".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // General instructions broadcast to all agents
    let gen_msg = ChatMessage::new_coordinator(
        "@all [General Instructions & Protocol]\nUniversal output protocol.".to_string(),
        None,
    );
    let gen_targets = app.determine_target_clients(&gen_msg);
    assert_eq!(gen_targets, vec![0, 1]);

    // Role instructions for Architect should ONLY target Architect (idx 0),
    // despite containing @Reviewer and @all in the instruction text.
    let arch_msg = ChatMessage::new_coordinator(
        "@Architect [Role Instructions: Lead Architect]\nLead architecture. Direct security to @Reviewer and announce to @all.".to_string(),
        None,
    );
    let arch_targets = app.determine_target_clients(&arch_msg);
    assert_eq!(arch_targets, vec![0]);

    // Role instructions for Reviewer should ONLY target Reviewer (idx 1),
    // despite containing @Architect in the instruction text.
    let rev_msg = ChatMessage::new_coordinator(
        "@Reviewer [Role Instructions: Reviewer]\nReview code. Direct questions to @Architect.".to_string(),
        None,
    );
    let rev_targets = app.determine_target_clients(&rev_msg);
    assert_eq!(rev_targets, vec![1]);

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}


#[tokio::test]
async fn test_coordinator_input_typing_and_navigation() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use mllm::app::AppState;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_typing_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![ClientConfig {
            number: 1,
            name: "Architect".to_string(),
            role: "Lead Architect".to_string(),
            command: "cat".to_string(),
            instructions: "".to_string(),
            extra_args: vec![],
        }],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // Initially focused on coordinator input with default responder
    assert_eq!(app.active_pane, 0);
    assert!(app.input_focused);
    assert_eq!(app.composer_input, "@Architect ");

    // Helper closure to create KeyEvent
    let make_key = |code, kind| KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind,
        state: KeyEventState::NONE,
    };

    // 1. Release event should be ignored
    let release_key = make_key(KeyCode::Char('x'), KeyEventKind::Release);
    assert!(!app.handle_key_event(release_key));
    assert_eq!(app.composer_input, "@Architect ");

    // 2. Typing characters
    for ch in "hello".chars() {
        let press = make_key(KeyCode::Char(ch), KeyEventKind::Press);
        app.handle_key_event(press);
    }
    assert_eq!(app.composer_input, "@Architect hello");

    // 3. Backspace
    app.handle_key_event(make_key(KeyCode::Backspace, KeyEventKind::Press));
    assert_eq!(app.composer_input, "@Architect hell");

    // 4. Cursor navigation (Left / Right / Home / End)
    app.handle_key_event(make_key(KeyCode::Left, KeyEventKind::Press));
    app.handle_key_event(make_key(KeyCode::Left, KeyEventKind::Press));
    // Cursor should now be before "ll"
    app.handle_key_event(make_key(KeyCode::Char('p'), KeyEventKind::Press));
    assert_eq!(app.composer_input, "@Architect hepll");

    app.handle_key_event(make_key(KeyCode::Home, KeyEventKind::Press));
    assert_eq!(app.cursor_pos, 0);

    app.handle_key_event(make_key(KeyCode::End, KeyEventKind::Press));
    assert_eq!(app.cursor_pos, app.composer_input.len());

    // 5. Unfocus with Esc, refocus with 'i'
    app.handle_key_event(make_key(KeyCode::Esc, KeyEventKind::Press));
    assert!(!app.input_focused);

    app.handle_key_event(make_key(KeyCode::Char('i'), KeyEventKind::Press));
    assert!(app.input_focused);

    // 6. Enter submits the message
    app.handle_key_event(make_key(KeyCode::Enter, KeyEventKind::Press));
    let last_msg = app.chat_history.last().unwrap();
    assert_eq!(last_msg.message, "@Architect hepll");
    // Composer reset to default responder
    assert_eq!(app.composer_input, "@Architect ");

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}


#[tokio::test]
async fn test_tab_autocomplete_available_agents() {
    use mllm::app::AppState;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_tab_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 3,
                name: "Researcher".to_string(),
                role: "Researcher".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // Scenario 1: User types '@' and presses Tab repeatedly
    app.composer_input = "@".to_string();
    app.cursor_pos = 1;
    app.tab_completion = None;

    // 1st Tab: completes to @Architect
    assert!(app.try_tab_complete(true));
    assert_eq!(app.composer_input, "@Architect ");
    assert_eq!(app.cursor_pos, "@Architect ".len());

    // 2nd Tab: cycles to @Reviewer
    assert!(app.try_tab_complete(true));
    assert_eq!(app.composer_input, "@Reviewer ");
    assert_eq!(app.cursor_pos, "@Reviewer ".len());

    // 3rd Tab: cycles to @Researcher
    assert!(app.try_tab_complete(true));
    assert_eq!(app.composer_input, "@Researcher ");
    assert_eq!(app.cursor_pos, "@Researcher ".len());

    // 4th Tab: cycles to @all
    assert!(app.try_tab_complete(true));
    assert_eq!(app.composer_input, "@all ");
    assert_eq!(app.cursor_pos, "@all ".len());

    // 5th Tab: wraps back to @Architect
    assert!(app.try_tab_complete(true));
    assert_eq!(app.composer_input, "@Architect ");

    // BackTab (reverse cycle): from Architect to all
    assert!(app.try_tab_complete(false));
    assert_eq!(app.composer_input, "@all ");

    // Scenario 2: Partial prefix autocomplete with '@Rev'
    app.composer_input = "@Rev".to_string();
    app.cursor_pos = 4;
    app.tab_completion = None;

    assert!(app.try_tab_complete(true));
    assert_eq!(app.composer_input, "@Reviewer ");

    // Scenario 3: Mid-sentence autocomplete '@Res'
    app.composer_input = "Please ask @Res for metrics".to_string();
    app.cursor_pos = 15; // directly after @Res
    app.tab_completion = None;

    assert!(app.try_tab_complete(true));
    assert_eq!(app.composer_input, "Please ask @Researcher  for metrics");

    // Scenario 4: Cursor NOT after '@'
    app.composer_input = "@Architect hello".to_string();
    app.cursor_pos = app.composer_input.len();
    app.tab_completion = None;

    // Should return false so Tab cycles panes instead
    assert!(!app.try_tab_complete(true));

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}


#[tokio::test]
async fn test_coordinator_multiline_input_and_navigation() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use mllm::app::AppState;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_multiline_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![ClientConfig {
            number: 1,
            name: "Architect".to_string(),
            role: "Lead Architect".to_string(),
            command: "cat".to_string(),
            instructions: "".to_string(),
            extra_args: vec![],
        }],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    let make_mod_key = |code, modifiers| KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };

    // 1. Shift+Enter inserts a newline
    app.composer_input = "@Architect line 1".to_string();
    app.cursor_pos = app.composer_input.len();
    app.handle_key_event(make_mod_key(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(app.composer_input, "@Architect line 1\n");
    assert_eq!(app.cursor_pos, "@Architect line 1\n".len());

    // Type "line 2"
    for ch in "line 2".chars() {
        app.handle_key_event(make_mod_key(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(app.composer_input, "@Architect line 1\nline 2");

    // 2. Alt+Enter inserts a newline
    app.handle_key_event(make_mod_key(KeyCode::Enter, KeyModifiers::ALT));
    assert_eq!(app.composer_input, "@Architect line 1\nline 2\n");

    // Type "line 3"
    for ch in "line 3".chars() {
        app.handle_key_event(make_mod_key(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(app.composer_input, "@Architect line 1\nline 2\nline 3");

    // 3. Up arrow navigates vertically across lines
    // Currently at end of "line 3" (col 6)
    app.handle_key_event(make_mod_key(KeyCode::Up, KeyModifiers::NONE));
    // Should be at end of "line 2"
    assert_eq!(app.cursor_pos, "@Architect line 1\nline 2".len());

    app.handle_key_event(make_mod_key(KeyCode::Up, KeyModifiers::NONE));
    // Should be on line 1 at col 6 ("@Archi")
    assert_eq!(app.cursor_pos, 6);

    app.handle_key_event(make_mod_key(KeyCode::Up, KeyModifiers::NONE));
    // Up on first line moves to start of text (0)
    assert_eq!(app.cursor_pos, 0);

    // 4. Down arrow navigates vertically down
    app.handle_key_event(make_mod_key(KeyCode::Down, KeyModifiers::NONE));
    // At col 0 of line 2: start of "line 2"
    assert_eq!(app.cursor_pos, "@Architect line 1\n".len());

    app.handle_key_event(make_mod_key(KeyCode::Down, KeyModifiers::NONE));
    // At col 0 of line 3: start of "line 3"
    assert_eq!(app.cursor_pos, "@Architect line 1\nline 2\n".len());

    app.handle_key_event(make_mod_key(KeyCode::Down, KeyModifiers::NONE));
    // Down on last line moves to end of text
    assert_eq!(app.cursor_pos, app.composer_input.len());

    // 5. Home and End line-level navigation
    app.handle_key_event(make_mod_key(KeyCode::Home, KeyModifiers::NONE));
    // Moves to start of current line ("line 3")
    assert_eq!(app.cursor_pos, "@Architect line 1\nline 2\n".len());

    app.handle_key_event(make_mod_key(KeyCode::Home, KeyModifiers::NONE));
    // Pressing Home again at line start moves to position 0
    assert_eq!(app.cursor_pos, 0);

    app.handle_key_event(make_mod_key(KeyCode::End, KeyModifiers::NONE));
    // Moves to end of first line
    assert_eq!(app.cursor_pos, "@Architect line 1".len());

    app.handle_key_event(make_mod_key(KeyCode::End, KeyModifiers::NONE));
    // Pressing End again at line end moves to end of entire text
    assert_eq!(app.cursor_pos, app.composer_input.len());

    // 6. Backspace across newline
    // Move to start of line 3
    app.cursor_pos = "@Architect line 1\nline 2\n".len();
    app.handle_key_event(make_mod_key(KeyCode::Backspace, KeyModifiers::NONE));
    // Backspacing the newline joins line 2 and line 3
    assert_eq!(app.composer_input, "@Architect line 1\nline 2line 3");

    // 7. Paste multi-line text with CRLF normalization
    app.composer_input.clear();
    app.cursor_pos = 0;
    app.handle_paste("@Architect\r\nFirst pasted line\nSecond pasted line");
    assert_eq!(
        app.composer_input,
        "@Architect\nFirst pasted line\nSecond pasted line"
    );

    // 8. Plain Enter submits the entire multi-line message
    app.handle_key_event(make_mod_key(KeyCode::Enter, KeyModifiers::NONE));
    let last_msg = app.chat_history.last().unwrap();
    assert_eq!(
        last_msg.message,
        "@Architect\nFirst pasted line\nSecond pasted line"
    );
    // Composer reset with default responder pre-filled
    assert_eq!(app.composer_input, "@Architect ");

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}


#[tokio::test]
async fn test_atomic_composite_prompt_lazy_injection() {
    use mllm::app::AppState;
    use mllm::chat::ChatMessage;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_lazy_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Create pre-existing chat log to simulate a warm start
    let chat_file = tmp_dir.join("test_chat.jsonl");
    let prior_msg = ChatMessage::new_user("Prior session question".to_string());
    let _ = mllm::chat::append_message(&chat_file, &prior_msg);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "Universal protocol.".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "Lead architecture design.".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer".to_string(),
                command: "cat".to_string(),
                instructions: "Audit system code.".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // Warm start: no startup instructions appended, initialized_clients is empty
    assert_eq!(app.chat_history.len(), 1);
    assert_eq!(app.initialized_clients.len(), 0);

    // 1. First task to Architect triggers lazy initialization
    let task1 = ChatMessage::new_user("@Architect Task one".to_string());
    app.route_message(&task1);
    assert!(app.initialized_clients.contains(&0));
    assert!(!app.initialized_clients.contains(&1));

    // 2. Second task to Architect: Architect already initialized
    let task2 = ChatMessage::new_user("@Architect Task two".to_string());
    app.route_message(&task2);
    assert!(app.initialized_clients.contains(&0));
    assert!(!app.initialized_clients.contains(&1));

    // 3. First task to Reviewer triggers Reviewer lazy initialization
    let task3 = ChatMessage::new_user("@Reviewer Audit this".to_string());
    app.route_message(&task3);
    assert!(app.initialized_clients.contains(&0));
    assert!(app.initialized_clients.contains(&1));

    // Shared chat history contains only the prior message (no synthetic coordinator rows injected)
    assert_eq!(app.chat_history.len(), 1);

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}




#[tokio::test]
async fn test_center_pane_mouse_selection_and_dual_tier_copy() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use mllm::app::AppState;
    use mllm::chat::ChatMessage;
    use mllm::config::{AppConfig, ClientConfig};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_center_sel_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let chat_file = tmp_dir.join("test_chat.jsonl");
    let msg1 = ChatMessage::new_user("First coordinator message for selection testing".to_string());
    let _ = mllm::chat::append_message(&chat_file, &msg1);

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // Render app on 100x30 test terminal to populate layout and coordinator_buffer
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| mllm::ui::render(f, &mut app)).unwrap();

    // Verify Pane 0 bounds are registered
    let (_, inner) = app.pane_bounds.get(&0).copied().expect("Pane 0 bounds should exist");
    assert!(inner.width > 0 && inner.height > 0);
    assert!(!app.coordinator_buffer.is_empty());

    // 1. Mouse down inside Pane 0 (Coordinator Pane)
    let click_pane0 = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: inner.x + 2,
        row: inner.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(click_pane0);
    assert_eq!(app.active_pane, 0);
    let sel = app.mouse_selection.clone().expect("Selection should be active in Pane 0");
    assert_eq!(sel.pane_number, 0);
    assert_eq!(sel.start, (2, 1));

    // 2. Mouse drag inside Pane 0
    let drag_pane0 = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: inner.x + 15,
        row: inner.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(drag_pane0);
    let sel = app.mouse_selection.clone().unwrap();
    assert_eq!(sel.end, (15, 1));

    // Redraw with selection: verifies visual highlight rendering without panic
    terminal.draw(|f| mllm::ui::render(f, &mut app)).unwrap();

    // 3. Mouse up: extracts text and copies to dual-tier clipboard
    let text = app.extract_selected_text(&sel);
    assert!(text.is_some());
    let extracted_str = text.unwrap();
    assert!(!extracted_str.is_empty());

    let up_pane0 = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: inner.x + 15,
        row: inner.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(up_pane0);

    // 4. Ctrl+C copies selection without quitting the application
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let should_quit = app.handle_key_event(ctrl_c);
    assert!(!should_quit, "Ctrl+C on active selection must copy and NOT quit");
    assert!(app.mouse_selection.is_none(), "Ctrl+C clears selection");

    // 5. Subsequent Ctrl+C with no selection triggers quit
    let should_quit_now = app.handle_key_event(ctrl_c);
    assert!(should_quit_now, "Ctrl+C without selection must quit");

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_realtime_thinking_indicator_lifecycle() {
    use mllm::app::AppState;
    use mllm::chat::ChatMessage;
    use mllm::config::{AppConfig, ClientConfig};
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_mllm_thinking_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);

    let chat_file = tmp_dir.join("test_chat.jsonl");

    let config = AppConfig {
        chat_file: "test_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "Protocol".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Reviewer".to_string(),
                role: "Reviewer & Auditor".to_string(),
                command: "cat".to_string(),
                instructions: "Audit".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // 1. Initial state: active_tasks is empty
    assert!(app.active_tasks.is_empty());

    // 2. Dispatch task from User to @Reviewer: active_tasks records Reviewer
    let user_msg = ChatMessage::new_user("@Reviewer Audit the memory safety".to_string());
    app.route_message(&user_msg);

    assert!(app.active_tasks.contains_key("reviewer"));
    let task = app.active_tasks.get("reviewer").unwrap();
    assert_eq!(task.agent_name, "Reviewer");
    assert_eq!(task.task_ref, user_msg.id);

    // 3. Reviewer writes response into shared_chat.jsonl
    let reply = ChatMessage {
        id: "reviewer-reply-1".to_string(),
        reply_to: Some(user_msg.id.clone()),
        sender: "Reviewer".to_string(),
        role: "Reviewer & Auditor".to_string(),
        message: "Memory safety audit completed. Zero leaks detected.".to_string(),
        timestamp: 1794057000,
    };
    mllm::chat::append_message(&chat_file, &reply).unwrap();

    // 4. Polling updates detects Reviewer's reply and clears active_tasks
    app.check_chat_log_updates();
    assert!(!app.active_tasks.contains_key("reviewer"));
    assert!(app.active_tasks.is_empty());

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_multi_agent_individual_finish_immediate_display() {
    use mllm::app::AppState;
    use mllm::chat::ChatMessage;
    use mllm::config::{AppConfig, ClientConfig};
    use std::fs;
    use tokio::sync::mpsc;

    let tmp_dir = std::env::temp_dir().join(format!("test_multi_agent_finish_{}", std::process::id()));
    let _ = fs::create_dir_all(&tmp_dir);
    let chat_file = tmp_dir.join("shared_chat.jsonl");

    let config = AppConfig {
        chat_file: "shared_chat.jsonl".to_string(),
        prefix_key: "ctrl+b".to_string(),
        general_instructions: "Protocol".to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "cat".to_string(),
                instructions: "Lead".to_string(),
                extra_args: vec![],
            },
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer & Auditor".to_string(),
                command: "cat".to_string(),
                instructions: "Audit".to_string(),
                extra_args: vec![],
            },
        ],
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = AppState::new(config, &tmp_dir, tx).expect("Failed to initialize AppState");

    // Dispatch message mentioning both agents
    let msg = ChatMessage::new_user("@Architect @Reviewer: Parallel evaluation".to_string());
    app.route_message(&msg);

    // Both agents are recorded as active tasks
    assert!(app.active_tasks.contains_key("architect"));
    assert!(app.active_tasks.contains_key("reviewer"));

    // Architect finishes first and appends response to shared_chat.jsonl
    let arch_reply = ChatMessage {
        id: "arch-reply-1".to_string(),
        reply_to: Some(msg.id.clone()),
        sender: "Architect".to_string(),
        role: "Lead Architect".to_string(),
        message: "Architecture verified.".to_string(),
        timestamp: 1794057100,
    };
    mllm::chat::append_message(&chat_file, &arch_reply).unwrap();

    // Check updates: Architect's output is immediately visible in chat_history,
    // Architect's active_task is cleared, but Reviewer is STILL active and thinking
    app.check_chat_log_updates();
    assert!(!app.active_tasks.contains_key("architect"));
    assert!(app.active_tasks.contains_key("reviewer"));
    assert!(app.chat_history.iter().any(|m| m.id == "arch-reply-1"));

    // Later, Reviewer finishes and appends response
    let rev_reply = ChatMessage {
        id: "rev-reply-1".to_string(),
        reply_to: Some(msg.id.clone()),
        sender: "Reviewer".to_string(),
        role: "Reviewer & Auditor".to_string(),
        message: "Review complete. All clear.".to_string(),
        timestamp: 1794057150,
    };
    mllm::chat::append_message(&chat_file, &rev_reply).unwrap();

    // Check updates: Reviewer's output is now also in chat_history,
    // and both active_tasks are now cleared
    app.check_chat_log_updates();
    assert!(!app.active_tasks.contains_key("reviewer"));
    assert!(app.active_tasks.is_empty());
    assert!(app.chat_history.iter().any(|m| m.id == "rev-reply-1"));

    app.shutdown();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_init_command_interactive_role_assignment() {
    use mllm::config::{load_config, run_init, AVAILABLE_CLIENTS};
    use std::fs;

    let tmp_dir = std::env::temp_dir().join(format!("test_integration_init_{}", std::process::id()));
    let _ = fs::create_dir_all(&tmp_dir);
    let target_yaml = tmp_dir.join("mllm.yaml");

    assert!(!target_yaml.exists());

    // User assigns:
    // Role 1 (Architect): claude
    // Role 2 (Reviewer): codex
    // Role 3 (Researcher): opencode
    // Role 4 (TechnicalWriter): agy
    // Role 5 (Designer): claude
    // Role 6 (Marketer): codex
    let mut user_inputs = "claude\ncodex\nopencode\nagy\nclaude\ncodex\n".as_bytes();
    let mut console_output = Vec::new();

    run_init(&mut user_inputs, &mut console_output, &target_yaml, false)
        .expect("run_init should succeed on non-existent mllm.yaml");

    assert!(target_yaml.exists(), "mllm.yaml must be created");

    let cfg = load_config(&target_yaml).expect("Generated mllm.yaml must be valid YAML");
    assert_eq!(cfg.chat_file, "shared_chat.jsonl");
    assert_eq!(cfg.clients.len(), 6);
    assert_eq!(cfg.clients[0].command, "claude");
    assert_eq!(cfg.clients[1].command, "codex");
    assert_eq!(cfg.clients[2].command, "opencode");
    assert_eq!(cfg.clients[3].command, "agy");
    assert_eq!(cfg.clients[4].command, "claude");
    assert_eq!(cfg.clients[5].command, "codex");

    for client in &cfg.clients {
        assert!(
            AVAILABLE_CLIENTS.contains(&client.command.as_str()),
            "Command '{}' must be one of the available LLM clients",
            client.command
        );
    }

    // Attempting to run init again without force skips recreation
    let mut repeat_input = "agy\nagy\nagy\nagy\nagy\nagy\n".as_bytes();
    let mut skip_output = Vec::new();
    run_init(&mut repeat_input, &mut skip_output, &target_yaml, false).unwrap();
    let skip_msg = String::from_utf8_lossy(&skip_output);
    assert!(skip_msg.contains("already exists in the project. Skipping creation"));

    // File content remains unchanged
    let unchanged_cfg = load_config(&target_yaml).unwrap();
    assert_eq!(unchanged_cfg.clients[0].command, "claude");

    // Overwriting with force = true
    let mut force_input = "agy\nagy\nagy\nagy\nagy\nagy\n".as_bytes();
    let mut force_output = Vec::new();
    run_init(&mut force_input, &mut force_output, &target_yaml, true).unwrap();
    let overwritten_cfg = load_config(&target_yaml).unwrap();
    assert_eq!(overwritten_cfg.clients[0].command, "agy");

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_update_workflow_dry_run_and_check() {
    use mllm::update::{run_update, UpdateOptions};

    let tmp_bin = std::env::temp_dir().join(format!("mock_mllm_{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp_bin, b"#!/bin/sh\necho mllm 0.1.0\n").unwrap();

    // 1. Dry run
    let mut out_dry = Vec::new();
    let opts_dry = UpdateOptions {
        dry_run: true,
        version: "v1.0.0".to_string(),
        target_path: Some(tmp_bin.clone()),
        ..Default::default()
    };
    let res = run_update(&opts_dry, &mut out_dry);
    assert!(res.is_ok(), "Dry-run update should succeed");
    let dry_str = String::from_utf8(out_dry).unwrap();
    assert!(dry_str.contains("[Dry Run] Planned actions:"));
    assert!(dry_str.contains("mllm-"));
    assert!(dry_str.contains(".tar.gz"));

    // 2. Check mode
    let mut out_check = Vec::new();
    let opts_check = UpdateOptions {
        check_only: true,
        version: format!("v{}", env!("CARGO_PKG_VERSION")),
        target_path: Some(tmp_bin.clone()),
        ..Default::default()
    };
    let res = run_update(&opts_check, &mut out_check);
    assert!(res.is_ok(), "Check-only update should succeed");
    let check_str = String::from_utf8(out_check).unwrap();
    assert!(check_str.contains("Current mllm version:"));

    let _ = std::fs::remove_file(&tmp_bin);
}

#[test]
fn test_update_atomic_replacement_integrity() {
    use mllm::update::atomic_replace;

    let tmp_dir = std::env::temp_dir().join(format!("test_atomic_integ_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let target = tmp_dir.join("mllm_live");
    let source = tmp_dir.join("mllm_staged");

    std::fs::write(&target, b"old-version-binary-data").unwrap();
    std::fs::write(&source, b"new-version-binary-data").unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old-version-binary-data");

    atomic_replace(&source, &target).expect("atomic_replace must succeed in writable directory");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new-version-binary-data");

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

