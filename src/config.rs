use anyhow::{anyhow, bail, Context, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_prefix")]
    pub prefix_key: String,
    #[serde(default)]
    pub general_instructions: String,
    /// Per-dispatch suffix appended to every routed task (see docs/task-dispatch-protocol.md).
    /// Placeholders: {chat_file} (alias {chatfile}), {id}, {client_name}, {client_role}.
    /// No built-in default: absent or empty means no suffix is appended; the text lives in
    /// the user's `mllm.yaml` (auto-scaffolded on first launch via `ensure_default_config_exists`,
    /// which seeds it from `SEED_MANDATORY_PROTOCOL`; `mllm.yaml.example` ships the same text).
    #[serde(default)]
    pub mandatory_protocol: String,
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

/// Seed text written into the first-launch scaffolded config (`ensure_default_config_exists`)
/// and shipped in `mllm.yaml.example`.
/// This is NOT a runtime default — the runtime has no built-in protocol; the user's
/// `mllm.yaml` is the sole source. Placeholders: `{chat_file}` (alias `{chatfile}`),
/// `{id}` (task ref), `{client_name}`, `{client_role}`.
pub const SEED_MANDATORY_PROTOCOL: &str = "[MANDATORY PROTOCOL]: Your turn is NOT complete until you append a valid single-line JSON object to {chat_file} with \"reply_to\": \"{id}\", \"sender\": \"{client_name}\", \"role\": \"{client_role}\". Terminal output alone is ignored by the coordinator.";

fn default_prefix() -> String {
    "alt".to_string()
}

fn default_command() -> String {
    "agy".to_string()
}

pub fn default_config_dir() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var("XDG_CONFIG_HOME").ok().filter(|s| !s.trim().is_empty()) {
        return Ok(PathBuf::from(xdg.trim()).join("mllm"));
    }
    if let Some(home) = std::env::var("HOME").ok().filter(|s| !s.trim().is_empty()) {
        return Ok(PathBuf::from(home.trim()).join(".config").join("mllm"));
    }
    bail!("Failed to determine default configuration directory: neither $XDG_CONFIG_HOME nor $HOME environment variable is set.")
}

pub fn default_config_path() -> Result<PathBuf> {
    Ok(default_config_dir()?.join("mllm.yaml"))
}

pub fn get_working_dir_dev_ino() -> Result<(u64, u64)> {
    let cwd = std::env::current_dir()
        .with_context(|| "Failed to determine current working directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(&cwd)
            .with_context(|| format!("Failed to read filesystem metadata for '{}'", cwd.display()))?;
        Ok((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        cwd.to_string_lossy().hash(&mut hasher);
        Ok((0, hasher.finish()))
    }
}

pub fn default_chat_log_dir() -> Result<PathBuf> {
    let (dev, ino) = get_working_dir_dev_ino()?;
    let conf_dir = default_config_dir()?;
    Ok(conf_dir.join("history").join(format!("{}_{}", dev, ino)))
}

pub fn default_chat_log_path() -> Result<PathBuf> {
    Ok(default_chat_log_dir()?.join("chat.jsonl"))
}

pub fn ensure_default_config_exists(config_path: &Path) -> Result<bool> {
    if config_path.exists() {
        return Ok(false);
    }
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    let default_config = default_init_config();
    let yaml_str = serde_yaml::to_string(&default_config)
        .with_context(|| "Failed to serialize default configuration to YAML")?;
    fs::write(config_path, yaml_str)
        .with_context(|| format!("Failed to write default configuration to '{}'", config_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(config_path, fs::Permissions::from_mode(0o600));
    }
    Ok(true)
}

pub fn ensure_chat_log_exists(chat_path: &Path) -> Result<()> {
    if let Some(parent) = chat_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create chat log directory: {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    if !chat_path.exists() {
        fs::File::create(chat_path)
            .with_context(|| format!("Failed to create chat log file: {}", chat_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(chat_path, fs::Permissions::from_mode(0o600));
        }
    }
    Ok(())
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            prefix_key: default_prefix(),
            general_instructions: String::new(),
            mandatory_protocol: String::new(),
            clients: Vec::new(),
        }
    }
}

impl AppConfig {
    /// Builds the standard variable interpolation map for client instruction templates.
    pub fn instruction_vars_for(&self, client: &ClientConfig) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        let chat_str = default_chat_log_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        vars.insert("chat_file".to_string(), chat_str);
        vars.insert("prefix_key".to_string(), self.prefix_key.clone());
        vars.insert("name".to_string(), client.name.clone());
        vars.insert("role".to_string(), client.role.clone());
        vars.insert("command".to_string(), client.command.clone());
        vars.insert("number".to_string(), client.number.to_string());
        vars
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedPrefix {
    pub modifiers: KeyModifiers,
    pub code: KeyCode,
}

impl ParsedPrefix {
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if self.code == KeyCode::Null {
            // Bare Alt mode (Kitty keyboard protocol): matches bare Alt modifier events
            return matches!(
                code,
                KeyCode::Modifier(
                    crossterm::event::ModifierKeyCode::LeftAlt
                        | crossterm::event::ModifierKeyCode::RightAlt
                )
            );
        }
        modifiers.contains(self.modifiers) && self.code == code
    }
}

pub fn parse_prefix_key(raw: &str) -> Result<ParsedPrefix> {
    let lower = raw.trim().to_lowercase();
    if lower == "alt" || lower == "meta" || lower == "opt" || lower == "option" {
        return Ok(ParsedPrefix {
            modifiers: KeyModifiers::ALT,
            code: KeyCode::Null,
        });
    }

    if let Some(rest) = lower
        .strip_prefix("alt+")
        .or_else(|| lower.strip_prefix("alt-"))
        .or_else(|| lower.strip_prefix("m-"))
    {
        let key_char = rest
            .chars()
            .next()
            .ok_or_else(|| anyhow!("Missing key character after alt prefix in '{}'", raw))?;
        return Ok(ParsedPrefix {
            modifiers: KeyModifiers::ALT,
            code: KeyCode::Char(key_char),
        });
    }

    bail!(
        "Invalid prefix_key configuration '{}'. Expected 'alt' or 'alt+<key>'.",
        raw
    )
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

pub fn default_init_config() -> AppConfig {
    AppConfig {
        prefix_key: default_prefix(),
        // Seed text for generated configs only — the runtime has no built-in protocol;
        // whatever is in the user's mllm.yaml is the sole source (empty = suffix disabled).
        mandatory_protocol: SEED_MANDATORY_PROTOCOL.to_string(),
        general_instructions: r#"Shared Log Coordination & Output Protocol:
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
                name: "TechnicalWriter".to_string(),
                role: "Technical Writer".to_string(),
                command: "agy".to_string(),
                instructions: r#"Author and maintain technical documentation, specifications, user guides, API docs, and release notes.
Ensure clarity, accuracy, completeness, and structured formatting across all project documentation.
Direct architectural clarification requests to @Architect in the shared log.
Direct accuracy and implementation verification queries to @Reviewer in the shared log.
"#.to_string(),
                extra_args: Vec::new(),
            },
            ClientConfig {
                number: 5,
                name: "Designer".to_string(),
                role: "Brand & UI/UX Designer".to_string(),
                command: "agy".to_string(),
                instructions: r#"Create design system components, wireframes, user journeys, and visual layouts.
Direct documentation layout and copy alignment requests to @TechnicalWriter.
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
Direct technical content and messaging adjustments to @TechnicalWriter.
Direct banner, landing page, and visual requirements to @Designer.
"#.to_string(),
                extra_args: Vec::new(),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prefix_key() {
        assert!(parse_prefix_key("ctrl+b").is_err());
        assert!(parse_prefix_key("Ctrl+B").is_err());
        assert!(parse_prefix_key("C-a").is_err());
        assert!(parse_prefix_key("ctrl-x").is_err());

        let p_alt = parse_prefix_key("alt").unwrap();
        assert_eq!(p_alt.code, KeyCode::Null);
        assert_eq!(p_alt.modifiers, KeyModifiers::ALT);
        // Alt+b does NOT trigger prefix toggle when prefix_key: "alt"
        assert!(!p_alt.matches(KeyCode::Char('b'), KeyModifiers::ALT));
        // Bare Alt modifier event DOES trigger prefix toggle
        assert!(p_alt.matches(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftAlt),
            KeyModifiers::ALT
        ));
        assert!(p_alt.matches(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::RightAlt),
            KeyModifiers::ALT
        ));

        let p_alt_b = parse_prefix_key("alt+b").unwrap();
        assert!(p_alt_b.matches(KeyCode::Char('b'), KeyModifiers::ALT));

        let p_alt_dash = parse_prefix_key("Alt-B").unwrap();
        assert!(p_alt_dash.matches(KeyCode::Char('b'), KeyModifiers::ALT));

        let p_meta = parse_prefix_key("m-x").unwrap();
        assert!(p_meta.matches(KeyCode::Char('x'), KeyModifiers::ALT));

        // Fail-closed checks on unknown formats
        assert!(parse_prefix_key("foobar").is_err());
        assert!(parse_prefix_key("invalid+key").is_err());
        assert!(parse_prefix_key("ctrl+").is_err());
        assert!(parse_prefix_key("alt-").is_err());
    }

    #[test]
    fn test_default_paths_and_scaffolding() {
        let conf_dir = default_config_dir().unwrap();
        assert!(conf_dir.ends_with(".config/mllm") || conf_dir.ends_with("mllm"));

        let conf_path = default_config_path().unwrap();
        assert!(conf_path.ends_with("mllm.yaml"));

        let log_path = default_chat_log_path().unwrap();
        assert!(log_path.ends_with("chat.jsonl"));
        assert!(log_path.to_string_lossy().contains("history"));

        let (dev, ino) = get_working_dir_dev_ino().unwrap();
        assert!(log_path.to_string_lossy().contains(&format!("{}_{}", dev, ino)));

        // Test scaffolding into a temporary directory
        let tmp_home = std::env::temp_dir().join(format!("test_home_{}", std::process::id()));
        let tmp_conf = tmp_home.join(".config").join("mllm").join("mllm.yaml");
        let created = ensure_default_config_exists(&tmp_conf).unwrap();
        assert!(created);
        assert!(tmp_conf.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let dir_mode = fs::metadata(tmp_conf.parent().unwrap()).unwrap().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
            let file_mode = fs::metadata(&tmp_conf).unwrap().mode() & 0o777;
            assert_eq!(file_mode, 0o600);
        }

        // Loading created config has default values
        let loaded = load_config(&tmp_conf).unwrap();
        assert_eq!(loaded.prefix_key, "alt");
        // Generated config seeds the editable mandatory protocol (placeholders intact).
        assert!(loaded.mandatory_protocol.contains("{chat_file}"));
        assert!(loaded.mandatory_protocol.contains("{client_name}"));

        // Subsequent call does not overwrite
        let created_again = ensure_default_config_exists(&tmp_conf).unwrap();
        assert!(!created_again);

        // Test chat log path creation
        let tmp_chat = tmp_home.join(".config").join("mllm").join("history").join("12345_67890").join("chat.jsonl");
        ensure_chat_log_exists(&tmp_chat).unwrap();
        assert!(tmp_chat.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let chat_dir_mode = fs::metadata(tmp_chat.parent().unwrap()).unwrap().mode() & 0o777;
            assert_eq!(chat_dir_mode, 0o700);
            let chat_file_mode = fs::metadata(&tmp_chat).unwrap().mode() & 0o777;
            assert_eq!(chat_file_mode, 0o600);
        }

        let _ = fs::remove_dir_all(&tmp_home);
    }

    #[test]
    fn test_instruction_vars_fixed_chat_file() {
        let (dev, ino) = get_working_dir_dev_ino().unwrap();
        let dev_ino = format!("{}_{}", dev, ino);

        let config = AppConfig::default();
        let client = ClientConfig {
            number: 1,
            name: "TestClient".to_string(),
            role: "Tester".to_string(),
            command: "agy".to_string(),
            instructions: String::new(),
            extra_args: Vec::new(),
        };
        let vars = config.instruction_vars_for(&client);
        let chat_var = vars.get("chat_file").unwrap();
        assert!(chat_var.contains(&dev_ino));
        assert!(chat_var.ends_with("chat.jsonl"));
    }

    #[test]
    fn test_mandatory_protocol_absent_means_disabled() {
        let yaml = r#"
prefix_key: "alt"
clients:
  - number: 1
    name: "Architect"
    role: "Lead Architect"
"#;
        let loaded: AppConfig = serde_yaml::from_str(yaml).unwrap();
        // No built-in default: absent field => empty => runtime appends no suffix.
        assert_eq!(loaded.mandatory_protocol, String::new());

        // The first-launch scaffolded config seeds the standard text (placeholders intact).
        let seeded = default_init_config();
        assert!(!seeded.mandatory_protocol.is_empty());
        assert!(seeded.mandatory_protocol.contains("{chat_file}"));
        assert!(seeded.mandatory_protocol.contains("{id}"));
        assert!(seeded.mandatory_protocol.contains("{client_name}"));
        assert!(seeded.mandatory_protocol.contains("{client_role}"));
    }

    #[test]
    fn test_mandatory_protocol_custom_template() {
        let yaml = r#"
prefix_key: "alt"
mandatory_protocol: 'Finish by writing to {chat_file} (task {id} for {client_name} / {client_role}).'
clients:
  - number: 1
    name: "Architect"
    role: "Lead Architect"
"#;
        let loaded: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            loaded.mandatory_protocol,
            "Finish by writing to {chat_file} (task {id} for {client_name} / {client_role})."
        );
    }

    #[test]
    fn test_config_collision_reassignment() {
        let yaml = r#"
prefix_key: "alt"
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
