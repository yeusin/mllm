use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub sender: String,
    pub role: String,
    pub message: String,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

impl ChatMessage {
    pub fn new_user(message: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            reply_to: None,
            sender: "User".to_string(),
            role: "Human Operator".to_string(),
            message,
            timestamp: current_timestamp(),
            fallback: None,
            superseded_by: None,
        }
    }

    pub fn new_coordinator(message: String, reply_to: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            reply_to,
            sender: "Coordinator".to_string(),
            role: "Coordinator".to_string(),
            message,
            timestamp: current_timestamp(),
            fallback: None,
            superseded_by: None,
        }
    }

    pub fn new_fallback(
        task_ref: &str,
        sender: String,
        role: String,
        message: String,
    ) -> Self {
        let deterministic_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("{}:{}:fallback", task_ref, sender).as_bytes(),
        )
        .to_string();
        Self {
            id: deterministic_id,
            reply_to: Some(task_ref.to_string()),
            sender,
            role,
            message,
            timestamp: current_timestamp(),
            fallback: Some(true),
            superseded_by: None,
        }
    }
}

pub fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn format_local_timestamp(timestamp: i64) -> String {
    DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "----/--/-- --:--:--".to_string())
}

pub fn load_messages(path: &Path) -> Result<(Vec<ChatMessage>, HashSet<String>)> {
    if !path.exists() {
        return Ok((Vec::new(), HashSet::new()));
    }

    let file = File::open(path)
        .with_context(|| format!("Failed to open chat log: {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut messages = Vec::new();
    let mut ids = HashSet::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(msg) = serde_json::from_str::<ChatMessage>(trimmed) {
            ids.insert(msg.id.clone());
            messages.push(msg);
        }
    }

    Ok((messages, ids))
}

pub fn load_new_messages_from_offset(
    path: &Path,
    last_offset: u64,
) -> Result<(Vec<ChatMessage>, u64)> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok((Vec::new(), 0));
    };

    let file_len = meta.len();
    if file_len == last_offset {
        return Ok((Vec::new(), last_offset));
    }

    if file_len < last_offset {
        // File was truncated or rotated: re-read from beginning
        let (messages, _) = load_messages(path)?;
        return Ok((messages, file_len));
    }

    let mut file = File::open(path)
        .with_context(|| format!("Failed to open chat log: {}", path.display()))?;

    file.seek(SeekFrom::Start(last_offset))?;
    let mut reader = BufReader::new(file);

    let mut messages = Vec::new();
    let mut current_offset = last_offset;
    let mut line_buf = String::new();

    loop {
        line_buf.clear();
        let bytes_read = reader.read_line(&mut line_buf)?;
        if bytes_read == 0 {
            break;
        }

        // Partial record at EOF: do not advance offset until writer flushes terminating newline
        if !line_buf.ends_with('\n') {
            break;
        }

        current_offset += bytes_read as u64;

        let trimmed = line_buf.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<ChatMessage>(trimmed) {
            Ok(msg) => messages.push(msg),
            Err(e) => {
                // R7: Salvage pass for fused records (e.g. crash mid-write without newline followed by next write)
                let mut salvaged = false;
                if trimmed.contains("}{") {
                    let parts: Vec<&str> = trimmed.split("}{").collect();
                    let mut temp_msgs = Vec::new();
                    let mut all_valid = true;
                    for (i, part) in parts.iter().enumerate() {
                        let json_str = if i == 0 {
                            format!("{}}}", part)
                        } else if i == parts.len() - 1 {
                            format!("{{{part}")
                        } else {
                            format!("{{{part}}}")
                        };
                        if let Ok(m) = serde_json::from_str::<ChatMessage>(&json_str) {
                            temp_msgs.push(m);
                        } else {
                            all_valid = false;
                            break;
                        }
                    }
                    if all_valid && !temp_msgs.is_empty() {
                        messages.extend(temp_msgs);
                        salvaged = true;
                    }
                }

                if !salvaged {
                    // Route corrupt records to dead-letter queue
                    let dead_letter_path = path.with_extension("dead_letter.jsonl");
                    if let Ok(mut dl_file) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&dead_letter_path)
                    {
                        let _ = writeln!(dl_file, "{}", trimmed);
                    }
                    eprintln!(
                        "Warning: Malformed JSONL line routed to {}: {}",
                        dead_letter_path.display(),
                        e
                    );
                }
            }
        }
    }

    Ok((messages, current_offset))
}

#[cfg(unix)]
struct FileLockGuard(std::os::unix::io::RawFd);

#[cfg(unix)]
impl Drop for FileLockGuard {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0, libc::LOCK_UN);
        }
    }
}

pub fn append_message(path: &Path, msg: &ChatMessage) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open chat file for appending: {}", path.display()))?;

    #[cfg(unix)]
    let _lock_guard = {
        let fd = file.as_raw_fd();
        unsafe {
            libc::flock(fd, libc::LOCK_EX);
        }
        FileLockGuard(fd)
    };

    let json = serde_json::to_string(msg)?;
    let mut payload = json.into_bytes();
    payload.push(b'\n');
    file.write_all(&payload)?;
    file.flush()?;

    Ok(())
}

static MENTION_STRIP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:@[a-zA-Z0-9_\-]+[:,]?[ \t]*)+").expect("Invalid mention strip regex")
});
static MULTI_SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+").expect("Invalid multi-space regex"));
static ROLE_INSTRUCTION_HEADER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*@([a-zA-Z0-9_\-]+)[:,]?\s+\[(?:Role (?:Instructions|Assignment)|Startup Instructions)[^\]]*\]")
        .expect("Invalid role instruction header regex")
});

static LINE_MENTION_PREFIX_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:(?:[-*>]|\d+\.)\s*(?:to\s+)?)?((?:@[a-zA-Z0-9_\-]+[:,]?[ \t]*)+)")
        .expect("Invalid line mention prefix regex")
});

static MENTION_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"@([a-zA-Z0-9_\-]+)").expect("Invalid mention token regex")
});

pub fn extract_role_instruction_target(message: &str) -> Option<String> {
    ROLE_INSTRUCTION_HEADER_REGEX
        .captures(message)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

pub fn extract_mention_target(message: &str) -> Option<String> {
    extract_mentions(message).into_iter().next()
}

pub fn extract_mentions(message: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    for line in message.lines() {
        if let Some(caps) = LINE_MENTION_PREFIX_REGEX.captures(line) {
            if let Some(prefix) = caps.get(1) {
                for cap in MENTION_TOKEN_REGEX.captures_iter(prefix.as_str()) {
                    if let Some(m) = cap.get(1) {
                        let name = m.as_str().to_string();
                        if !mentions
                            .iter()
                            .any(|existing: &String| existing.eq_ignore_ascii_case(&name))
                        {
                            mentions.push(name);
                        }
                    }
                }
            }
        }
    }
    mentions
}

pub fn strip_at_mentions(text: &str) -> String {
    let removed = MENTION_STRIP_REGEX.replace(text, "");
    let mut lines = Vec::new();
    for line in removed.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            lines.push(String::new());
        } else {
            let collapsed = MULTI_SPACE_REGEX.replace_all(trimmed, " ");
            lines.push(collapsed.to_string());
        }
    }
    lines.join("\n").trim().to_string()
}

/// Replaces variable placeholders formatted as `{variable}` or `{ variable }` with corresponding values from `vars`.
/// Unmatched curly braces (such as JSON syntax like `{"id": ...}`) are safely preserved.
pub fn replace_instruction_placeholders(
    template: &str,
    vars: &HashMap<String, String>,
) -> String {
    let mut result = template.to_string();
    for (k, v) in vars {
        result = result.replace(&format!("{{{}}}", k), v);
        result = result.replace(&format!("{{ {} }}", k), v);
    }
    result
}

pub fn build_startup_instructions(
    general_instructions: &str,
    client_name: &str,
    client_role: &str,
    client_instructions: &str,
) -> String {
    let vars = HashMap::new();
    build_startup_instructions_with_vars(
        general_instructions,
        client_name,
        client_role,
        client_instructions,
        &vars,
    )
}

pub fn build_startup_instructions_with_vars(
    general_instructions: &str,
    client_name: &str,
    client_role: &str,
    client_instructions: &str,
    custom_vars: &HashMap<String, String>,
) -> String {
    let mut vars = custom_vars.clone();
    vars.entry("name".to_string()).or_insert_with(|| client_name.to_string());
    vars.entry("client_name".to_string()).or_insert_with(|| client_name.to_string());
    vars.entry("role".to_string()).or_insert_with(|| client_role.to_string());
    vars.entry("client_role".to_string()).or_insert_with(|| client_role.to_string());

    let general_trimmed = replace_instruction_placeholders(general_instructions.trim(), &vars);
    let inst = replace_instruction_placeholders(client_instructions.trim(), &vars);

    let mut sections = Vec::new();
    if !general_trimmed.is_empty() {
        sections.push(format!("[General Instructions & Protocol]\n{}", general_trimmed));
    }

    let mut role_section = format!(
        "[Role Instructions]\nName: {}\nRole: {}",
        client_name, client_role
    );
    if !inst.is_empty() {
        role_section.push_str(&format!("\n{}", inst));
    }
    sections.push(role_section);

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_mentions() {
        // Mentions in the middle of the text are not extracted
        let msg_mid = "Hello @Architect and @Reviewer, please check this. Also @all.";
        assert_eq!(extract_mentions(msg_mid), Vec::<String>::new());

        // Mentions at the beginning are extracted (single target only)
        let msg_single = "@Architect please check this";
        assert_eq!(extract_mentions(msg_single), vec!["Architect"]);

        // Multiple mentions: only the first one at the beginning is extracted
        let msg_multi = "@Architect please coordinate with @Reviewer and @all";
        assert_eq!(extract_mentions(msg_multi), vec!["Architect"]);

        // Broadcast at beginning
        let msg_all = "@all evaluate porting multi-llm to Rust";
        assert_eq!(extract_mentions(msg_all), vec!["all"]);

        // With leading whitespace
        let msg_space = "  @Reviewer please review";
        assert_eq!(extract_mentions(msg_space), vec!["Reviewer"]);

        // With punctuation directly after mention
        let msg_colon = "@Architect: evaluate performance";
        assert_eq!(extract_mentions(msg_colon), vec!["Architect"]);
        let msg_comma = "@Architect, evaluate performance";
        assert_eq!(extract_mentions(msg_comma), vec!["Architect"]);

        // Multiline mentions (each on its own line)
        let msg_multiline = "[Report]\nSome text\n@Reviewer please review\n@Researcher please benchmark";
        assert_eq!(extract_mentions(msg_multiline), vec!["Reviewer", "Researcher"]);

        // Multiple consecutive mentions on the same leading line
        let msg_same_line = "@User @Reviewer @Researcher Interactive init command";
        assert_eq!(extract_mentions(msg_same_line), vec!["User", "Reviewer", "Researcher"]);

        // Markdown list directives
        let msg_directives = "2. Directives to Peers:\n   - To @Reviewer: Audit input sanitization\n   - To @Researcher: Benchmark setup";
        assert_eq!(extract_mentions(msg_directives), vec!["Reviewer", "Researcher"]);

        let msg_bullet = "- @Reviewer: please check\n* @Researcher: please bench";
        assert_eq!(extract_mentions(msg_bullet), vec!["Reviewer", "Researcher"]);

        // Invalid mentions or empty
        assert_eq!(extract_mentions(""), Vec::<String>::new());
        assert_eq!(extract_mentions("@"), Vec::<String>::new());
        assert_eq!(extract_mentions("@@Architect"), Vec::<String>::new());

        // extract_mention_target returns Option<String>
        assert_eq!(extract_mention_target(msg_single), Some("Architect".to_string()));
        assert_eq!(extract_mention_target(msg_mid), None);
    }

    #[test]
    fn test_extract_role_instruction_target() {
        let msg1 = "@Architect [Role Instructions: Lead Architect]\nLead design. Mention @Reviewer.";
        assert_eq!(
            extract_role_instruction_target(msg1),
            Some("Architect".to_string())
        );

        let msg2 = "@Reviewer [Role Assignment: Auditor]";
        assert_eq!(
            extract_role_instruction_target(msg2),
            Some("Reviewer".to_string())
        );

        let msg_startup = "@Architect [Startup Instructions: Lead Architect]\nUniversal protocol.";
        assert_eq!(
            extract_role_instruction_target(msg_startup),
            Some("Architect".to_string())
        );

        let msg3 = "@all [General Instructions & Protocol]\nShared log protocol";
        assert_eq!(extract_role_instruction_target(msg3), None);

        let msg4 = "@Architect please audit @Reviewer code";
        assert_eq!(extract_role_instruction_target(msg4), None);
    }

    #[test]
    fn test_strip_at_mentions() {
        assert_eq!(
            strip_at_mentions("@Architect please check this"),
            "please check this"
        );
        assert_eq!(
            strip_at_mentions("@all [General Instructions & Protocol]\nSome text"),
            "[General Instructions & Protocol]\nSome text"
        );
        assert_eq!(
            strip_at_mentions("@Architect: evaluate performance"),
            "evaluate performance"
        );
        assert_eq!(
            strip_at_mentions("@Architect @Reviewer please review"),
            "please review"
        );
        assert_eq!(strip_at_mentions("@Architect"), "");
        assert_eq!(
            strip_at_mentions("@Architect please coordinate with @Reviewer and @all"),
            "please coordinate with @Reviewer and @all"
        );
        assert_eq!(
            strip_at_mentions(
                "@Reviewer [Role Instructions: Reviewer & Auditor]\nReview architecture.\nDirect architectural redesign questions to @Architect in the shared log."
            ),
            "[Role Instructions: Reviewer & Auditor]\nReview architecture.\nDirect architectural redesign questions to @Architect in the shared log."
        );
    }

    #[test]
    fn test_chat_message_roundtrip() {
        let tmp_file = std::env::temp_dir().join(format!("test_chat_{}.jsonl", std::process::id()));
        let msg1 = ChatMessage::new_user("Test message 1".to_string());
        append_message(&tmp_file, &msg1).unwrap();

        let msg2 = ChatMessage::new_coordinator("Test message 2".to_string(), Some(msg1.id.clone()));
        append_message(&tmp_file, &msg2).unwrap();

        let (messages, ids) = load_messages(&tmp_file).unwrap();
        let _ = std::fs::remove_file(&tmp_file);

        assert_eq!(messages.len(), 2);
        assert_eq!(ids.len(), 2);
        assert_eq!(messages[0].id, msg1.id);
        assert_eq!(messages[0].message, "Test message 1");
        assert_eq!(messages[1].id, msg2.id);
        assert_eq!(messages[1].reply_to, Some(msg1.id));
    }

    #[test]
    fn test_build_startup_instructions() {
        let general = "Output in JSONL format.";
        let name = "Architect";
        let role = "Lead Architect";
        let instructions = "Lead the design.";

        let startup = build_startup_instructions(general, name, role, instructions);
        assert!(startup.contains("[General Instructions & Protocol]"));
        assert!(startup.contains("Output in JSONL format."));
        assert!(startup.contains("[Role Instructions]"));
        assert!(startup.contains("Name: Architect"));
        assert!(startup.contains("Role: Lead Architect"));
        assert!(startup.contains("Lead the design."));
    }

    #[test]
    fn test_instruction_placeholders_replacement() {
        let mut vars = HashMap::new();
        vars.insert("chat_file".to_string(), "custom_chat.jsonl".to_string());
        vars.insert("prefix_key".to_string(), "ctrl+a".to_string());
        vars.insert("name".to_string(), "Reviewer".to_string());
        vars.insert("role".to_string(), "Auditor".to_string());

        let template = "Log to {chat_file} (or { chat_file }) using {prefix_key}. Example JSON: {\"id\": \"<uuid>\"}. Client {name} as {role}.";
        let result = replace_instruction_placeholders(template, &vars);

        assert_eq!(
            result,
            "Log to custom_chat.jsonl (or custom_chat.jsonl) using ctrl+a. Example JSON: {\"id\": \"<uuid>\"}. Client Reviewer as Auditor."
        );
    }

    #[test]
    fn test_build_startup_instructions_with_vars() {
        let general = "Shared Log: Append to {chat_file}. Schema: {\"id\": \"<uuid>\"}.";
        let name = "Researcher";
        let role = "Investigator";
        let instructions = "You are {name}, tasked with {role}. Report to {chat_file}.";

        let mut vars = HashMap::new();
        vars.insert("chat_file".to_string(), "team_log.jsonl".to_string());

        let startup = build_startup_instructions_with_vars(general, name, role, instructions, &vars);
        assert!(startup.contains("Shared Log: Append to team_log.jsonl."));
        assert!(startup.contains("Schema: {\"id\": \"<uuid>\"}."));
        assert!(startup.contains("You are Researcher, tasked with Investigator. Report to team_log.jsonl."));
    }

    #[test]
    fn test_format_local_timestamp() {
        let ts = 1730956200;
        let formatted = format_local_timestamp(ts);
        assert_eq!(formatted.len(), 19);
        assert!(formatted.contains('-'));
        assert!(formatted.contains(':'));
    }

    #[test]
    fn test_load_new_messages_from_offset() {
        let tmp_file = std::env::temp_dir().join(format!("test_chat_offset_{}.jsonl", std::process::id()));
        let msg1 = ChatMessage::new_user("Initial Message".to_string());
        append_message(&tmp_file, &msg1).unwrap();

        let (initial_msgs, offset1) = load_new_messages_from_offset(&tmp_file, 0).unwrap();
        assert_eq!(initial_msgs.len(), 1);
        assert_eq!(initial_msgs[0].id, msg1.id);
        assert!(offset1 > 0);

        // At offset1, there are 0 new messages
        let (no_msgs, offset2) = load_new_messages_from_offset(&tmp_file, offset1).unwrap();
        assert_eq!(no_msgs.len(), 0);
        assert_eq!(offset2, offset1);

        // Append msg2
        let msg2 = ChatMessage::new_coordinator("Incremental Message".to_string(), Some(msg1.id.clone()));
        append_message(&tmp_file, &msg2).unwrap();

        // Incrementally load only msg2
        let (new_msgs, offset3) = load_new_messages_from_offset(&tmp_file, offset1).unwrap();
        assert_eq!(new_msgs.len(), 1);
        assert_eq!(new_msgs[0].id, msg2.id);
        assert!(offset3 > offset1);

        // Test partial record write at EOF:
        let partial_json = r#"{"id": "partial-1", "sender": "User", "role": "Operator", "message": "Incomp"#;
        {
            let mut file = OpenOptions::new().append(true).open(&tmp_file).unwrap();
            file.write_all(partial_json.as_bytes()).unwrap();
            file.flush().unwrap();
        }
        // Should NOT advance offset past the incomplete line and should return 0 messages
        let (partial_msgs, offset_stuck) = load_new_messages_from_offset(&tmp_file, offset3).unwrap();
        assert_eq!(partial_msgs.len(), 0);
        assert_eq!(offset_stuck, offset3);

        // Complete the line with newline
        let completion = r#"lete message", "timestamp": 12345}"#;
        {
            let mut file = OpenOptions::new().append(true).open(&tmp_file).unwrap();
            writeln!(file, "{}", completion).unwrap();
            file.flush().unwrap();
        }

        // Now it successfully parses the completed record and advances offset!
        let (completed_msgs, offset_completed) = load_new_messages_from_offset(&tmp_file, offset3).unwrap();
        assert_eq!(completed_msgs.len(), 1);
        assert_eq!(completed_msgs[0].id, "partial-1");
        assert_eq!(completed_msgs[0].message, "Incomplete message");
        assert!(offset_completed > offset3);

        let _ = std::fs::remove_file(&tmp_file);
    }
}
