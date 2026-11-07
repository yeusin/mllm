use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_chat_file")]
    pub chat_file: String,
    #[serde(default = "default_prefix")]
    pub prefix_key: String,
    #[serde(default)]
    pub general_instructions: String,
    #[serde(default)]
    pub clients: Vec<ClientConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub number: usize,
    pub name: String,
    pub role: String,
    #[serde(default = "default_command")]
    pub command: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

fn default_chat_file() -> String {
    "shared_chat.jsonl".to_string()
}

fn default_prefix() -> String {
    "ctrl+b".to_string()
}

fn default_command() -> String {
    "agy".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            chat_file: default_chat_file(),
            prefix_key: default_prefix(),
            general_instructions: String::new(),
            clients: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedPrefix {
    pub modifiers: KeyModifiers,
    pub code: KeyCode,
}

impl ParsedPrefix {
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.code == code && modifiers.contains(self.modifiers)
    }
}

pub fn parse_prefix_key(raw: &str) -> ParsedPrefix {
    let lower = raw.trim().to_lowercase();
    // Normalize forms like "ctrl+b", "ctrl-b", "c-b"
    let key_char = if let Some(rest) = lower.strip_prefix("ctrl+") {
        rest.chars().next().unwrap_or('b')
    } else if let Some(rest) = lower.strip_prefix("ctrl-") {
        rest.chars().next().unwrap_or('b')
    } else if let Some(rest) = lower.strip_prefix("c-") {
        rest.chars().next().unwrap_or('b')
    } else {
        lower.chars().next().unwrap_or('b')
    };

    ParsedPrefix {
        modifiers: KeyModifiers::CONTROL,
        code: KeyCode::Char(key_char),
    }
}

pub fn load_config(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read configuration file: {}", path.display()))?;
    let mut config: AppConfig = serde_yaml::from_str(&contents)
        .with_context(|| format!("Failed to parse YAML configuration: {}", path.display()))?;

    // Validate and normalize client numbers:
    // 0 is reserved for Coordinator. Ensure unique numbers between 1 and 9.
    let mut used_numbers = HashSet::new();
    used_numbers.insert(0); // Reserved

    for client in &mut config.clients {
        if client.number == 0 || used_numbers.contains(&client.number) || client.number > 9 {
            // Find lowest available positive integer 1..9
            let mut candidate = 1;
            while used_numbers.contains(&candidate) && candidate <= 9 {
                candidate += 1;
            }
            client.number = candidate;
        }
        used_numbers.insert(client.number);
    }

    // Sort clients by pane number
    config.clients.sort_by_key(|c| c.number);

    Ok(config)
}

/// Supported LLM clients for role assignment
pub const AVAILABLE_CLIENTS: &[&str] = &["claude", "codex", "opencode", "agy"];

pub fn normalize_client_choice(input: &str, default: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(default.to_string());
    }
    match trimmed {
        "1" => Some("claude".to_string()),
        "2" => Some("codex".to_string()),
        "3" => Some("opencode".to_string()),
        "4" => Some("agy".to_string()),
        other => {
            let lower = other.to_lowercase();
            for &client in AVAILABLE_CLIENTS {
                if lower == client || lower.starts_with(&format!("{} ", client)) {
                    return Some(other.to_string());
                }
            }
            None
        }
    }
}

pub fn default_init_config() -> AppConfig {
    AppConfig {
        chat_file: default_chat_file(),
        prefix_key: default_prefix(),
        general_instructions: r#"Shared Log Coordination & Output Protocol:
1. Shared Log File: All clients coordinate and store outputs by appending to shared_chat.jsonl.
2. Output Storage in Shared Log: Write each output as a single valid JSON line followed by a newline:
   {"id": "<uuid>", "reply_to": "<question_uuid_or_null>", "sender": "<YourName>", "role": "<YourRole>", "message": "<Content with optional @Peer>", "timestamp": <unix_timestamp>}
3. Unique UUID & Question Linking: Generate a unique UUID for "id" and reference the question's UUID with "reply_to".
4. Unix Timestamp: The "timestamp" field must be an integer Unix timestamp in epoch seconds.
5. Directing Tasks & Questions: Mention @all to broadcast to all available agents, or mention peers using @AgentName (@Architect, @Reviewer, @Researcher).
6. Mention Routing: The Center Coordinator monitors the shared log and automatically delivers @all or @mentioned tasks to target agents.
7. Communication Standards: Be concise, clear, and actionable.
"#.to_string(),
        clients: vec![
            ClientConfig {
                number: 1,
                name: "Architect".to_string(),
                role: "Lead Architect".to_string(),
                command: "agy".to_string(),
                instructions: r#"Lead system architecture, technical specifications, runtime decisions, and interface contracts.
Evaluate system language choices, concurrency patterns, and framework tradeoffs.
Direct security, edge cases, and audit concerns to @Reviewer in the shared log.
Direct ecosystem, crate, and benchmark investigations to @Researcher in the shared log.
"#.to_string(),
                extra_args: Vec::new(),
            },
            ClientConfig {
                number: 2,
                name: "Reviewer".to_string(),
                role: "Reviewer & Auditor".to_string(),
                command: "opencode".to_string(),
                instructions: r#"Review architecture, code quality, edge cases, vulnerability vectors, and concurrency safety.
Audit reliability, error handling, memory safety, and maintenance overhead.
Direct architectural redesign questions to @Architect in the shared log.
Direct benchmark verification requests to @Researcher in the shared log.
"#.to_string(),
                extra_args: Vec::new(),
            },
            ClientConfig {
                number: 3,
                name: "Researcher".to_string(),
                role: "Researcher".to_string(),
                command: "agy".to_string(),
                instructions: r#"Conduct technical research, performance profiling, library evaluations, and ecosystem comparisons.
Analyze cross-platform tooling, crate/package benchmarks, and developer velocity tradeoffs.
Direct architectural implications to @Architect in the shared log.
Direct compliance and security findings to @Reviewer in the shared log.
"#.to_string(),
                extra_args: Vec::new(),
            },
            ClientConfig {
                number: 4,
                name: "Copywriter".to_string(),
                role: "Content Strategist & Copywriter".to_string(),
                command: "agy".to_string(),
                instructions: r#"Develop product narratives, landing page copy, value propositions, and social campaigns.
Direct visual layout and mockup requests to @Designer.
Direct audience targeting and conversion queries to @Marketer.
"#.to_string(),
                extra_args: Vec::new(),
            },
            ClientConfig {
                number: 5,
                name: "Designer".to_string(),
                role: "Brand & UI/UX Designer".to_string(),
                command: "agy".to_string(),
                instructions: r#"Create design system components, wireframes, user journeys, and visual layouts.
Direct copy and headline alignment requests to @Copywriter.
Direct campaign asset specifications to @Marketer.
"#.to_string(),
                extra_args: Vec::new(),
            },
            ClientConfig {
                number: 6,
                name: "Marketer".to_string(),
                role: "Growth & Campaign Specialist".to_string(),
                command: "agy".to_string(),
                instructions: r#"Evaluate user acquisition channels, audience segmentation, conversion funnels, and KPIs.
Direct narrative and messaging adjustments to @Copywriter.
Direct banner, landing page, and visual requirements to @Designer.
"#.to_string(),
                extra_args: Vec::new(),
            },
        ],
    }
}

pub fn prompt_client_choice<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    role_name: &str,
    role_desc: &str,
    default_client: &str,
) -> Result<String> {
    writeln!(writer, "\nAssign LLM client for role '{}' ({}):", role_name, role_desc)?;
    for (i, client) in AVAILABLE_CLIENTS.iter().enumerate() {
        writeln!(writer, "  {}) {}", i + 1, client)?;
    }
    loop {
        write!(writer, "Select client [1-4 or name] (default: {}): ", default_client)?;
        writer.flush()?;

        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF reached, use default
            return Ok(default_client.to_string());
        }

        if let Some(chosen) = normalize_client_choice(&line, default_client) {
            return Ok(chosen);
        }

        writeln!(
            writer,
            "Invalid choice '{}'. Please select 1-4, claude, codex, opencode, or agy.",
            line.trim()
        )?;
    }
}

pub fn run_init<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    output_path: &Path,
    force: bool,
) -> Result<()> {
    if output_path.exists() && !force {
        writeln!(
            writer,
            "Configuration file '{}' already exists in the project. Skipping creation (use --force to overwrite).",
            output_path.display()
        )?;
        return Ok(());
    }

    let example_path = output_path
        .parent()
        .map(|p| p.join("mllm.yaml.example"))
        .filter(|p| p.exists())
        .or_else(|| {
            let p = PathBuf::from("mllm.yaml.example");
            if p.exists() {
                Some(p)
            } else {
                None
            }
        });

    let mut config = if let Some(ref path) = example_path {
        load_config(path).unwrap_or_else(|_| default_init_config())
    } else {
        default_init_config()
    };

    writeln!(
        writer,
        "Initializing Multi-LLM configuration for '{}'...",
        output_path.display()
    )?;

    for client in &mut config.clients {
        let current_default = AVAILABLE_CLIENTS
            .iter()
            .find(|c| client.command.to_lowercase().starts_with(*c))
            .copied()
            .unwrap_or("agy");

        let selected = prompt_client_choice(
            reader,
            writer,
            &client.name,
            &client.role,
            current_default,
        )?;
        client.command = selected;
    }

    let yaml_str = serde_yaml::to_string(&config)
        .with_context(|| "Failed to serialize configuration to YAML")?;

    fs::write(output_path, yaml_str)
        .with_context(|| format!("Failed to write configuration file to '{}'", output_path.display()))?;

    writeln!(
        writer,
        "\nSuccessfully created '{}' with assigned LLM clients.",
        output_path.display()
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_client_choice() {
        assert_eq!(normalize_client_choice("1", "agy").unwrap(), "claude");
        assert_eq!(normalize_client_choice("2", "agy").unwrap(), "codex");
        assert_eq!(normalize_client_choice("3", "agy").unwrap(), "opencode");
        assert_eq!(normalize_client_choice("4", "agy").unwrap(), "agy");
        assert_eq!(normalize_client_choice("claude", "agy").unwrap(), "claude");
        assert_eq!(normalize_client_choice("Codex", "agy").unwrap(), "Codex");
        assert_eq!(normalize_client_choice("OPENCODE", "agy").unwrap(), "OPENCODE");
        assert_eq!(normalize_client_choice("agy", "claude").unwrap(), "agy");
        assert_eq!(normalize_client_choice("", "opencode").unwrap(), "opencode");
        assert_eq!(normalize_client_choice("   ", "agy").unwrap(), "agy");
        assert_eq!(normalize_client_choice("claude --dangerously-skip-permissions", "agy").unwrap(), "claude --dangerously-skip-permissions");
        assert!(normalize_client_choice("unsupported_client", "agy").is_none());
    }

    #[test]
    fn test_run_init_workflow() {
        let tmp_dir = std::env::temp_dir().join(format!("test_init_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let out_yaml = tmp_dir.join("mllm.yaml");

        // Simulate user assigning:
        // 1: claude
        // 2: codex
        // 3: opencode
        // 4: agy
        // 5: claude
        // 6: default (empty string)
        let mut input = "1\n2\n3\n4\nclaude\n\n".as_bytes();
        let mut output = Vec::new();

        run_init(&mut input, &mut output, &out_yaml, false).expect("run_init failed");
        assert!(out_yaml.exists());

        let cfg = load_config(&out_yaml).expect("load_config on generated file failed");
        assert_eq!(cfg.clients.len(), 6);
        assert_eq!(cfg.clients[0].command, "claude");
        assert_eq!(cfg.clients[1].command, "codex");
        assert_eq!(cfg.clients[2].command, "opencode");
        assert_eq!(cfg.clients[3].command, "agy");
        assert_eq!(cfg.clients[4].command, "claude");

        // Verify skipping when already exists and force is false
        let mut skip_output = Vec::new();
        let mut empty_input = "".as_bytes();
        run_init(&mut empty_input, &mut skip_output, &out_yaml, false).expect("skip check failed");
        let skip_msg = String::from_utf8_lossy(&skip_output);
        assert!(skip_msg.contains("already exists in the project. Skipping creation"));

        // Cleanup
        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_parse_prefix_key() {
        let p1 = parse_prefix_key("ctrl+b");
        assert!(p1.matches(KeyCode::Char('b'), KeyModifiers::CONTROL));

        let p2 = parse_prefix_key("Ctrl+B");
        assert!(p2.matches(KeyCode::Char('b'), KeyModifiers::CONTROL));

        let p3 = parse_prefix_key("C-a");
        assert!(p3.matches(KeyCode::Char('a'), KeyModifiers::CONTROL));

        let p4 = parse_prefix_key("ctrl-x");
        assert!(p4.matches(KeyCode::Char('x'), KeyModifiers::CONTROL));
    }

    #[test]
    fn test_config_collision_reassignment() {
        let yaml = r#"
chat_file: "test_chat.jsonl"
prefix_key: "ctrl+b"
clients:
  - number: 0
    name: "ClientZero"
    role: "Zero"
  - number: 1
    name: "ClientOne"
    role: "One"
  - number: 1
    name: "ClientDuplicate"
    role: "Duplicate"
"#;
        let tmp_file = std::env::temp_dir().join(format!("test_config_{}.yaml", std::process::id()));
        std::fs::write(&tmp_file, yaml).unwrap();

        let cfg = load_config(&tmp_file).unwrap();
        let _ = std::fs::remove_file(&tmp_file);

        assert_eq!(cfg.clients.len(), 3);
        assert!(!cfg.clients.iter().any(|c| c.number == 0));
        let mut numbers: Vec<usize> = cfg.clients.iter().map(|c| c.number).collect();
        numbers.sort();
        assert_eq!(numbers, vec![1, 2, 3]);
    }

    #[test]
    fn test_load_example_configs() {
        let example = Path::new("mllm.yaml.example");
        if example.exists() {
            let cfg = load_config(example).unwrap();
            assert!(!cfg.clients.is_empty());
            assert_eq!(cfg.clients[0].name, "Architect");
        }
    }
}
