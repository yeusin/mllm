use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

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
    #[serde(default)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let eng = Path::new("examples/mllm.engineering.yaml");
        if eng.exists() {
            let cfg = load_config(eng).unwrap();
            assert_eq!(cfg.clients.len(), 3);
            assert_eq!(cfg.clients[0].name, "Architect");
        }

        let mkt = Path::new("examples/mllm.marketing_design.yaml");
        if mkt.exists() {
            let cfg = load_config(mkt).unwrap();
            assert_eq!(cfg.clients.len(), 3);
        }
    }
}
