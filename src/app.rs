use crate::chat::{
    append_message, build_startup_instructions_with_vars, extract_mentions,
    extract_role_instruction_target, load_messages, strip_at_mentions, ChatMessage,
};
use crate::config::{parse_prefix_key, AppConfig, ParsedPrefix};
use crate::pty::ClientPane;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSelection {
    pub pane_number: usize,
    pub start: (u16, u16), // relative to inner pane rect: (col, row)
    pub end: (u16, u16),
}

#[derive(Debug, Clone)]
pub struct TabCompletionState {
    pub original_prefix: String,
    pub at_index: usize,
    pub matches: Vec<String>,
    pub current_idx: usize,
    pub last_replaced_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTask {
    pub agent_name: String,
    pub agent_role: String,
    pub task_ref: String,
    pub start_time: std::time::Instant,
}

pub struct AppState {
    pub config: AppConfig,
    pub parsed_prefix: ParsedPrefix,
    pub active_pane: usize, // 0 = Coordinator, 1..9 = client number
    pub prefix_active: bool,
    pub input_focused: bool,
    pub composer_input: String,
    pub cursor_pos: usize,
    pub chat_history: Vec<ChatMessage>,
    pub known_message_ids: HashSet<String>,
    pub initial_message_ids: HashSet<String>,
    pub clients: Vec<ClientPane>,
    pub active_chain_depth: usize,
    pub max_chain_depth: usize,
    pub chat_file_path: PathBuf,
    pub pty_dimensions: HashMap<usize, (u16, u16)>,
    pub collaborative_next_idx: usize,
    pub tab_completion: Option<TabCompletionState>,
    pub initialized_clients: HashSet<usize>,
    pub mouse_selection: Option<PaneSelection>,
    pub pane_bounds: HashMap<usize, (Rect, Rect)>, // pane_number -> (outer_rect, inner_rect)
    pub chat_scroll_offset: usize,
    pub pane_scroll_offsets: HashMap<usize, usize>,
    pub last_chat_offset: u64,
    pub coordinator_buffer: Vec<Vec<String>>,
    pub clipboard: Option<Arc<Mutex<arboard::Clipboard>>>,
    pub active_tasks: HashMap<String, ActiveTask>,
    pub render_cache: HashMap<String, (usize, Vec<ratatui::text::Line<'static>>)>,
    pub chat_cache_width: usize,
    pub chat_total_lines: usize,
    pub message_heights: Vec<usize>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        custom_chat_path: Option<&Path>,
        event_tx: UnboundedSender<()>,
    ) -> Result<Self> {
        let parsed_prefix = parse_prefix_key(&config.prefix_key)?;
        let chat_file_path = match custom_chat_path {
            Some(p) => {
                if p.is_dir() {
                    p.join("chat.jsonl")
                } else {
                    p.to_path_buf()
                }
            }
            None => crate::config::default_chat_log_path()?,
        };

        // Ensure shared chat log file exists on disk (creates it on first run)
        crate::config::ensure_chat_log_exists(&chat_file_path)?;

        // Load existing chat history
        let (chat_history, initial_message_ids) = load_messages(&chat_file_path)?;
        let initial_offset = std::fs::metadata(&chat_file_path).map(|m| m.len()).unwrap_or(0);
        let known_message_ids = initial_message_ids.clone();

        // Spawn clients in mllm's current working directory
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        // Initialize client panes
        let mut clients = Vec::new();
        for client_cfg in &config.clients {
            let pane = ClientPane::new(
                client_cfg.number,
                client_cfg.name.clone(),
                client_cfg.role.clone(),
                &client_cfg.command,
                &client_cfg.extra_args,
                &working_dir,
                80,
                24,
                event_tx.clone(),
            )?;
            clients.push(pane);
        }

        // Unified Lazy Instruction Injection:
        // Startup instructions are not delivered via chat file replay or startup broadcasts.
        // Even when chat_file is empty or not created, all agents receive general protocol
        // and role-specific instructions lazily prepended to their first task prompt as an
        // atomic composite prompt.
        let initialized_clients = HashSet::new();

        let mut state = Self {
            config,
            parsed_prefix,
            active_pane: 0,
            prefix_active: false,
            input_focused: true,
            composer_input: String::new(),
            cursor_pos: 0,
            chat_history,
            known_message_ids,
            initial_message_ids,
            clients,
            active_chain_depth: 0,
            max_chain_depth: 100,
            chat_file_path,
            pty_dimensions: HashMap::new(),
            collaborative_next_idx: 0,
            tab_completion: None,
            initialized_clients,
            mouse_selection: None,
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: initial_offset,
            coordinator_buffer: Vec::new(),
            clipboard: arboard::Clipboard::new().ok().map(|cb| Arc::new(Mutex::new(cb))),
            active_tasks: HashMap::new(),
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };
        state.reset_composer_with_default();

        Ok(state)
    }

    pub fn sync_pty_size(&mut self, client_idx: usize, cols: u16, rows: u16) {
        if let Some(client) = self.clients.get_mut(client_idx) {
            let current = self.pty_dimensions.get(&client_idx).copied();
            if current != Some((cols, rows)) {
                client.resize(cols, rows);
                self.pty_dimensions.insert(client_idx, (cols, rows));
            }
        }
    }

    pub fn copy_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }

        #[cfg(target_os = "linux")]
        if let Some(cb_mutex) = &self.clipboard {
            if let Ok(mut cb) = cb_mutex.lock() {
                use arboard::{LinuxClipboardKind, SetExtLinux};
                let _ = cb
                    .set()
                    .clipboard(LinuxClipboardKind::Clipboard)
                    .text(text.to_owned());
                let _ = cb
                    .set()
                    .clipboard(LinuxClipboardKind::Primary)
                    .text(text.to_owned());
            }
        }

        #[cfg(not(target_os = "linux"))]
        if let Some(cb_mutex) = &self.clipboard {
            if let Ok(mut cb) = cb_mutex.lock() {
                let _ = cb.set_text(text);
            }
        }

        copy_to_clipboard_osc52(text);
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        // Ignore key release events from terminals supporting release reporting
        if key.kind == crossterm::event::KeyEventKind::Release {
            return false;
        }

        let code = key.code;
        let modifiers = key.modifiers;

        // Selection Copy via Ctrl+C (intercept before global quit if selection is active)
        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
            if let Some(selection) = &self.mouse_selection {
                if selection.start != selection.end {
                    if let Some(text) = self.extract_selected_text(selection) {
                        if !text.is_empty() {
                            self.copy_text(&text);
                        }
                    }
                    self.mouse_selection = None;
                    return false;
                }
            }
        }

        // Global Quit: Ctrl+C or Ctrl+Q (when no active text selection)
        if modifiers.contains(KeyModifiers::CONTROL)
            && (code == KeyCode::Char('c') || code == KeyCode::Char('q'))
        {
            return true;
        }

        // Instant Pane Switch & Shortcuts: Alt+0 .. Alt+9, Alt+o / Alt+n, Alt+p, Alt+y
        if modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(c) = code
                && let Some(digit) = c.to_digit(10) {
                    let target_pane = digit as usize;
                    if target_pane == 0 || self.clients.iter().any(|c| c.number == target_pane) {
                        self.active_pane = target_pane;
                        if target_pane == 0 {
                            self.input_focused = true;
                        }
                        self.prefix_active = false;
                        return false;
                    }
                }

            match code {
                KeyCode::Char('o') | KeyCode::Char('n') => {
                    self.cycle_pane(true);
                    return false;
                }
                KeyCode::Char('p') => {
                    self.cycle_pane(false);
                    return false;
                }
                KeyCode::Char('y') => {
                    if let Some(text) = self.mouse_selection.as_ref()
                        .filter(|s| s.start != s.end)
                        .and_then(|s| self.extract_selected_text(s))
                        .filter(|t| !t.is_empty())
                    {
                        self.copy_text(&text);
                    }
                    return false;
                }
                _ => {}
            }
        }

        // Prefix Mode Handling
        if self.prefix_active {
            match code {
                KeyCode::Char('0') => {
                    self.active_pane = 0;
                    self.input_focused = true;
                    self.prefix_active = false;
                    return false;
                }
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    if let Some(digit) = c.to_digit(10) {
                        let num = digit as usize;
                        if self.clients.iter().any(|c| c.number == num) {
                            self.active_pane = num;
                        }
                    }
                    self.prefix_active = false;
                    return false;
                }
                KeyCode::Char('o') | KeyCode::Char('n') => {
                    self.cycle_pane(true);
                    self.prefix_active = false;
                    return false;
                }
                KeyCode::Char('p') => {
                    self.cycle_pane(false);
                    self.prefix_active = false;
                    return false;
                }
                KeyCode::Char('y') => {
                    if let Some(selection) = &self.mouse_selection {
                        if selection.start != selection.end {
                            if let Some(text) = self.extract_selected_text(selection) {
                                if !text.is_empty() {
                                    self.copy_text(&text);
                                }
                            }
                        }
                    }
                    self.prefix_active = false;
                    return false;
                }
                KeyCode::Esc => {
                    self.prefix_active = false;
                    self.mouse_selection = None;
                    return false;
                }
                _ => {
                    self.prefix_active = false;
                }
            }
        }

        // Check for Tmux Prefix activation (e.g. Ctrl+B)
        if self.parsed_prefix.matches(code, modifiers) {
            self.prefix_active = true;
            return false;
        }

        // Tab / Shift+Tab pane cycling, autocomplete, or responder cycling
        if code == KeyCode::Tab {
            if self.active_pane == 0 && self.input_focused {
                if self.try_tab_complete(true) || self.cycle_responder(true) {
                    return false;
                }
            }
            self.tab_completion = None;
            self.cycle_pane(true);
            return false;
        }
        if code == KeyCode::BackTab {
            if self.active_pane == 0 && self.input_focused {
                if (self.tab_completion.is_some() && self.try_tab_complete(false))
                    || self.cycle_responder(false)
                {
                    return false;
                }
            }
            self.tab_completion = None;
            self.cycle_pane(false);
            return false;
        }

        // PageUp / PageDown scrollback history
        if code == KeyCode::PageUp {
            if self.active_pane == 0 {
                self.scroll_up(10);
                return false;
            } else {
                self.scroll_pane_up(self.active_pane, 10);
                return false;
            }
        }
        if code == KeyCode::PageDown {
            if self.active_pane == 0 {
                self.scroll_down(10);
                return false;
            } else {
                self.scroll_pane_down(self.active_pane, 10);
                return false;
            }
        }

        if code == KeyCode::Esc {
            self.mouse_selection = None;
            self.chat_scroll_offset = 0;
            if self.active_pane != 0 {
                self.scroll_pane_down(self.active_pane, usize::MAX);
            }
        }

        // Mode-specific dispatch
        if self.active_pane == 0 {
            self.handle_coordinator_input(code, modifiers);
        } else {
            self.handle_client_input(code, modifiers);
        }

        false
    }

    fn cycle_pane(&mut self, forward: bool) {
        let mut pane_numbers = vec![0];
        for c in &self.clients {
            pane_numbers.push(c.number);
        }
        if pane_numbers.is_empty() {
            return;
        }

        let current_pos = pane_numbers
            .iter()
            .position(|&p| p == self.active_pane)
            .unwrap_or(0);

        let next_pos = if forward {
            (current_pos + 1) % pane_numbers.len()
        } else {
            (current_pos + pane_numbers.len() - 1) % pane_numbers.len()
        };

        self.active_pane = pane_numbers[next_pos];
        if self.active_pane == 0 {
            self.input_focused = true;
        }
    }

    pub fn try_tab_complete(&mut self, forward: bool) -> bool {
        // If an active tab completion session exists:
        if let Some(ref mut state) = self.tab_completion {
            if !state.matches.is_empty() {
                let replace_start = state.at_index + 1;
                let replace_end = replace_start + state.last_replaced_len;
                if replace_end <= self.composer_input.len() && self.cursor_pos == replace_end {
                    let next_idx = if forward {
                        (state.current_idx + 1) % state.matches.len()
                    } else {
                        (state.current_idx + state.matches.len() - 1) % state.matches.len()
                    };

                    let before = self.composer_input[..replace_start].to_string();
                    let after = self.composer_input[replace_end..].to_string();
                    let next_name = &state.matches[next_idx];
                    let replacement = format!("{} ", next_name);

                    self.composer_input = format!("{}{}{}", before, replacement, after);
                    self.cursor_pos = replace_start + replacement.len();

                    state.current_idx = next_idx;
                    state.last_replaced_len = replacement.len();
                    return true;
                }
            }
            self.tab_completion = None;
        }

        if self.cursor_pos > self.composer_input.len()
            || !self.composer_input.is_char_boundary(self.cursor_pos)
        {
            return false;
        }

        let at_idx = match self.composer_input[..self.cursor_pos].rfind('@') {
            Some(idx) => idx,
            None => return false,
        };

        let typed = self.composer_input[at_idx + 1..self.cursor_pos].to_string();
        if typed.chars().any(char::is_whitespace) {
            return false;
        }

        let mut candidates = Vec::new();
        for client in &self.clients {
            if !candidates
                .iter()
                .any(|c: &String| c.eq_ignore_ascii_case(&client.name))
            {
                candidates.push(client.name.clone());
            }
        }
        if !candidates.iter().any(|c: &String| c.eq_ignore_ascii_case("all")) {
            candidates.push("all".to_string());
        }

        let matches: Vec<String> = candidates
            .into_iter()
            .filter(|c| c.to_lowercase().starts_with(&typed.to_lowercase()))
            .collect();

        if matches.is_empty() {
            return false;
        }

        let replace_start = at_idx + 1;
        let replace_end = self.cursor_pos;
        let before = self.composer_input[..replace_start].to_string();
        let after = self.composer_input[replace_end..].to_string();

        let first_name = &matches[0];
        let replacement = format!("{} ", first_name);

        self.composer_input = format!("{}{}{}", before, replacement, after);
        self.cursor_pos = replace_start + replacement.len();

        self.tab_completion = Some(TabCompletionState {
            original_prefix: typed.to_string(),
            at_index: at_idx,
            matches,
            current_idx: 0,
            last_replaced_len: replacement.len(),
        });

        true
    }

    pub fn handle_paste(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.active_pane == 0 && self.input_focused {
            if self.cursor_pos > self.composer_input.len()
                || !self.composer_input.is_char_boundary(self.cursor_pos)
            {
                self.cursor_pos = self.composer_input.len();
            }
            self.composer_input.insert_str(self.cursor_pos, &normalized);
            self.cursor_pos += normalized.len();
        } else if let Some(client) = self.clients.iter().find(|c| c.number == self.active_pane) {
            let _ = client.write_raw(normalized.as_bytes());
        }
    }

    pub fn handle_mouse_event(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Find which pane was clicked based on outer rectangle
                let clicked = self.pane_bounds.iter().find_map(|(&pane_num, &(outer, inner))| {
                    if event.column >= outer.x
                        && event.column < outer.right()
                        && event.row >= outer.y
                        && event.row < outer.bottom()
                    {
                        Some((pane_num, inner))
                    } else {
                        None
                    }
                });

                if let Some((pane_num, inner)) = clicked {
                    self.active_pane = pane_num;
                    if pane_num == 0 {
                        self.input_focused = true;
                    } else {
                        self.input_focused = false;
                    }
                    self.prefix_active = false;

                    if inner.width > 0 && inner.height > 0 {
                        let col = event
                            .column
                            .clamp(inner.x, inner.right().saturating_sub(1))
                            .saturating_sub(inner.x);
                        let row = event
                            .row
                            .clamp(inner.y, inner.bottom().saturating_sub(1))
                            .saturating_sub(inner.y);

                        self.mouse_selection = Some(PaneSelection {
                            pane_number: pane_num,
                            start: (col, row),
                            end: (col, row),
                        });
                    } else {
                        self.mouse_selection = None;
                    }
                } else {
                    self.mouse_selection = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = &mut self.mouse_selection {
                    if let Some(&(_, inner)) = self.pane_bounds.get(&selection.pane_number) {
                        if inner.width > 0 && inner.height > 0 {
                            let col = event
                                .column
                                .clamp(inner.x, inner.right().saturating_sub(1))
                                .saturating_sub(inner.x);
                            let row = event
                                .row
                                .clamp(inner.y, inner.bottom().saturating_sub(1))
                                .saturating_sub(inner.y);

                            selection.end = (col, row);
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(selection) = &self.mouse_selection {
                    if selection.start != selection.end {
                        if let Some(text) = self.extract_selected_text(selection) {
                            if !text.is_empty() {
                                self.copy_text(&text);
                            }
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) | MouseEventKind::Up(MouseButton::Right) => {
                if let Some(selection) = &self.mouse_selection {
                    if selection.start != selection.end {
                        if let Some(text) = self.extract_selected_text(selection) {
                            if !text.is_empty() {
                                self.copy_text(&text);
                            }
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                let hovered = self.pane_bounds.iter().find_map(|(&pane_num, &(outer, _))| {
                    if event.column >= outer.x
                        && event.column < outer.right()
                        && event.row >= outer.y
                        && event.row < outer.bottom()
                    {
                        Some(pane_num)
                    } else {
                        None
                    }
                });
                if let Some(pane_num) = hovered {
                    if pane_num == 0 {
                        self.scroll_up(3);
                    } else {
                        self.scroll_pane_up(pane_num, 3);
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                let hovered = self.pane_bounds.iter().find_map(|(&pane_num, &(outer, _))| {
                    if event.column >= outer.x
                        && event.column < outer.right()
                        && event.row >= outer.y
                        && event.row < outer.bottom()
                    {
                        Some(pane_num)
                    } else {
                        None
                    }
                });
                if let Some(pane_num) = hovered {
                    if pane_num == 0 {
                        self.scroll_down(3);
                    } else {
                        self.scroll_pane_down(pane_num, 3);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn extract_selected_text(&self, selection: &PaneSelection) -> Option<String> {
        if selection.pane_number == 0 {
            if self.coordinator_buffer.is_empty() {
                None
            } else {
                Some(extract_grid_text(
                    &self.coordinator_buffer,
                    selection.start,
                    selection.end,
                ))
            }
        } else {
            let client = self.clients.iter().find(|c| c.number == selection.pane_number)?;
            let parser = client.vt_parser.lock().ok()?;
            let screen = parser.screen();
            Some(extract_screen_text(screen, selection.start, selection.end))
        }
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.chat_scroll_offset = self.chat_scroll_offset.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.chat_scroll_offset = self.chat_scroll_offset.saturating_sub(lines);
    }

    pub fn scroll_pane_up(&mut self, pane_number: usize, lines: usize) {
        if let Some(client) = self.clients.iter().find(|c| c.number == pane_number) {
            if let Ok(mut parser) = client.vt_parser.lock() {
                let max_scroll = parser.screen().scrollback_len();
                let current = parser.screen().scrollback();
                let new_offset = (current + lines).min(max_scroll);
                parser.screen_mut().set_scrollback(new_offset);
                self.pane_scroll_offsets.insert(pane_number, new_offset);
            }
        }
    }

    pub fn scroll_pane_down(&mut self, pane_number: usize, lines: usize) {
        if let Some(client) = self.clients.iter().find(|c| c.number == pane_number) {
            if let Ok(mut parser) = client.vt_parser.lock() {
                let current = parser.screen().scrollback();
                let new_offset = current.saturating_sub(lines);
                parser.screen_mut().set_scrollback(new_offset);
                self.pane_scroll_offsets.insert(pane_number, new_offset);
            }
        }
    }

    pub fn cycle_responder(&mut self, forward: bool) -> bool {
        let mut candidates = Vec::new();
        for client in &self.clients {
            if !candidates.iter().any(|c: &String| c.eq_ignore_ascii_case(&client.name)) {
                candidates.push(client.name.clone());
            }
        }
        if !candidates.iter().any(|c: &String| c.eq_ignore_ascii_case("all")) {
            candidates.push("all".to_string());
        }
        if candidates.is_empty() {
            return false;
        }

        if self.composer_input.starts_with('@') {
            let end_of_mention = self.composer_input[1..]
                .find(|c: char| c.is_whitespace() || c == ':')
                .map(|idx| idx + 1)
                .unwrap_or(self.composer_input.len());

            let current_name = &self.composer_input[1..end_of_mention];
            let current_idx = candidates
                .iter()
                .position(|c| c.eq_ignore_ascii_case(current_name));

            let next_idx = match current_idx {
                Some(idx) => {
                    if forward {
                        (idx + 1) % candidates.len()
                    } else {
                        (idx + candidates.len() - 1) % candidates.len()
                    }
                }
                None => 0,
            };

            let next_name = &candidates[next_idx];
            let mut rest_start = end_of_mention;
            if rest_start < self.composer_input.len() && self.composer_input[rest_start..].starts_with(':') {
                rest_start += 1;
            }
            while rest_start < self.composer_input.len()
                && self.composer_input[rest_start..].starts_with(' ')
            {
                rest_start += 1;
            }

            let rest = &self.composer_input[rest_start..];
            let new_prefix = format!("@{} ", next_name);
            let old_len = rest_start;
            let new_len = new_prefix.len();

            self.composer_input = format!("{}{}", new_prefix, rest);
            if self.cursor_pos <= old_len {
                self.cursor_pos = new_len;
            } else {
                self.cursor_pos = self.cursor_pos.saturating_sub(old_len) + new_len;
            }
            true
        } else {
            let first_name = &candidates[0];
            let prefix = format!("@{} ", first_name);
            let prefix_len = prefix.len();
            self.composer_input = format!("{}{}", prefix, self.composer_input);
            self.cursor_pos += prefix_len;
            true
        }
    }

    pub fn handle_coordinator_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.tab_completion = None;
        if self.input_focused {
            let is_newline = (code == KeyCode::Enter
                && (modifiers.contains(KeyModifiers::SHIFT)
                    || modifiers.contains(KeyModifiers::ALT)
                    || modifiers.contains(KeyModifiers::CONTROL)))
                || (modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('j'))
                || code == KeyCode::Char('\n');

            if is_newline {
                if self.cursor_pos > self.composer_input.len()
                    || !self.composer_input.is_char_boundary(self.cursor_pos)
                {
                    self.cursor_pos = self.composer_input.len();
                }
                self.composer_input.insert(self.cursor_pos, '\n');
                self.cursor_pos += 1;
                return;
            }

            match code {
                KeyCode::Enter => {
                    let text = self.composer_input.trim().to_string();
                    if !text.is_empty() {
                        self.submit_user_message(text);
                    }
                }
                KeyCode::Esc => {
                    self.input_focused = false;
                }
                KeyCode::Backspace => {
                    if self.cursor_pos > 0 {
                        let mut prev = self.cursor_pos - 1;
                        while prev > 0 && !self.composer_input.is_char_boundary(prev) {
                            prev -= 1;
                        }
                        self.composer_input.drain(prev..self.cursor_pos);
                        self.cursor_pos = prev;
                    }
                }
                KeyCode::Delete => {
                    if self.cursor_pos < self.composer_input.len() {
                        let mut next = self.cursor_pos + 1;
                        while next < self.composer_input.len() && !self.composer_input.is_char_boundary(next) {
                            next += 1;
                        }
                        self.composer_input.drain(self.cursor_pos..next);
                    }
                }
                KeyCode::Left => {
                    if self.cursor_pos > 0 {
                        let mut prev = self.cursor_pos - 1;
                        while prev > 0 && !self.composer_input.is_char_boundary(prev) {
                            prev -= 1;
                        }
                        self.cursor_pos = prev;
                    }
                }
                KeyCode::Right => {
                    if self.cursor_pos < self.composer_input.len() {
                        let mut next = self.cursor_pos + 1;
                        while next < self.composer_input.len() && !self.composer_input.is_char_boundary(next) {
                            next += 1;
                        }
                        self.cursor_pos = next;
                    }
                }
                KeyCode::Up => {
                    let current_line_start = self.composer_input[..self.cursor_pos]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let col_chars = self.composer_input[current_line_start..self.cursor_pos]
                        .chars()
                        .count();

                    if current_line_start > 0 {
                        let prev_newline = current_line_start - 1;
                        let prev_line_start = self.composer_input[..prev_newline]
                            .rfind('\n')
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        let mut target = prev_line_start;
                        for (char_idx, ch) in self.composer_input[prev_line_start..prev_newline]
                            .chars()
                            .enumerate()
                        {
                            if char_idx >= col_chars {
                                break;
                            }
                            target += ch.len_utf8();
                        }
                        self.cursor_pos = target;
                    } else {
                        self.cursor_pos = 0;
                    }
                }
                KeyCode::Down => {
                    let current_line_start = self.composer_input[..self.cursor_pos]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let col_chars = self.composer_input[current_line_start..self.cursor_pos]
                        .chars()
                        .count();

                    if let Some(next_newline_offset) = self.composer_input[self.cursor_pos..].find('\n') {
                        let next_line_start = self.cursor_pos + next_newline_offset + 1;
                        let next_line_end = self.composer_input[next_line_start..]
                            .find('\n')
                            .map(|i| next_line_start + i)
                            .unwrap_or(self.composer_input.len());
                        let mut target = next_line_start;
                        for (char_idx, ch) in self.composer_input[next_line_start..next_line_end]
                            .chars()
                            .enumerate()
                        {
                            if char_idx >= col_chars {
                                break;
                            }
                            target += ch.len_utf8();
                        }
                        self.cursor_pos = target;
                    } else {
                        self.cursor_pos = self.composer_input.len();
                    }
                }
                KeyCode::Home => {
                    let line_start = self.composer_input[..self.cursor_pos]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    if self.cursor_pos == line_start {
                        self.cursor_pos = 0;
                    } else {
                        self.cursor_pos = line_start;
                    }
                }
                KeyCode::End => {
                    let line_end = self.composer_input[self.cursor_pos..]
                        .find('\n')
                        .map(|i| self.cursor_pos + i)
                        .unwrap_or(self.composer_input.len());
                    if self.cursor_pos == line_end {
                        self.cursor_pos = self.composer_input.len();
                    } else {
                        self.cursor_pos = line_end;
                    }
                }
                KeyCode::Char(c) => {
                    if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) {
                        return;
                    }
                    if self.cursor_pos > self.composer_input.len()
                        || !self.composer_input.is_char_boundary(self.cursor_pos)
                    {
                        self.cursor_pos = self.composer_input.len();
                    }
                    self.composer_input.insert(self.cursor_pos, c);
                    self.cursor_pos += c.len_utf8();
                }
                _ => {}
            }
        } else {
            // Navigation mode (composer unfocused)
            match code {
                KeyCode::Char('0') => {
                    self.input_focused = true;
                }
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    if let Some(digit) = c.to_digit(10) {
                        let num = digit as usize;
                        if self.clients.iter().any(|c| c.number == num) {
                            self.active_pane = num;
                        }
                    }
                }
                KeyCode::Char('i') | KeyCode::Enter => {
                    self.input_focused = true;
                }
                _ => {}
            }
        }
    }

    fn handle_client_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.pane_scroll_offsets.get(&self.active_pane).copied().unwrap_or(0) > 0 {
            self.scroll_pane_down(self.active_pane, usize::MAX);
        }

        if let Some(client) = self.clients.iter().find(|c| c.number == self.active_pane) {
            let mut bytes = Vec::new();
            if modifiers.contains(KeyModifiers::CONTROL) {
                if let KeyCode::Char(c) = code {
                    let ctrl_byte = (c.to_ascii_lowercase() as u8).saturating_sub(b'a' - 1);
                    bytes.push(ctrl_byte);
                }
            } else {
                match code {
                    KeyCode::Enter => bytes.push(b'\r'),
                    KeyCode::Backspace => bytes.push(0x7f),
                    KeyCode::Tab => bytes.push(b'\t'),
                    KeyCode::Esc => bytes.push(0x1b),
                    KeyCode::Up => bytes.extend_from_slice(b"\x1b[A"),
                    KeyCode::Down => bytes.extend_from_slice(b"\x1b[B"),
                    KeyCode::Right => bytes.extend_from_slice(b"\x1b[C"),
                    KeyCode::Left => bytes.extend_from_slice(b"\x1b[D"),
                    KeyCode::Home => bytes.extend_from_slice(b"\x1b[H"),
                    KeyCode::End => bytes.extend_from_slice(b"\x1b[F"),
                    KeyCode::PageUp => bytes.extend_from_slice(b"\x1b[5~"),
                    KeyCode::PageDown => bytes.extend_from_slice(b"\x1b[6~"),
                    KeyCode::Delete => bytes.extend_from_slice(b"\x1b[3~"),
                    KeyCode::Char(c) => {
                        let mut b = [0u8; 4];
                        bytes.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                    }
                    _ => {}
                }
            }
            if !bytes.is_empty() {
                let _ = client.write_raw(&bytes);
            }
        }
    }

    pub fn default_responder(&self) -> Option<&str> {
        self.clients
            .iter()
            .min_by_key(|c| c.number)
            .map(|c| c.name.as_str())
    }

    pub fn composer_targets(&self) -> (Vec<String>, bool) {
        let mentions = extract_mentions(&self.composer_input);
        if !mentions.is_empty() {
            (mentions, false)
        } else if let Some(default_name) = self.default_responder() {
            (vec![default_name.to_string()], true)
        } else {
            (Vec::new(), false)
        }
    }

    pub fn reset_composer_with_default(&mut self) {
        if let Some(default_name) = self.default_responder() {
            self.composer_input = format!("@{} ", default_name);
            self.cursor_pos = self.composer_input.len();
        } else {
            self.composer_input.clear();
            self.cursor_pos = 0;
        }
    }

    pub fn submit_user_message(&mut self, text: String) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }

        // Force at least one responder using @{name}. Default is the earliest numbered agent if not specified.
        let mentions = extract_mentions(trimmed);
        let final_text = if mentions.is_empty() {
            if let Some(default_name) = self.default_responder() {
                format!("@{} {}", default_name, trimmed)
            } else {
                trimmed.to_string()
            }
        } else {
            trimmed.to_string()
        };

        // If after stripping mentions there is no message content, do not send
        let content_without_mentions = strip_at_mentions(&final_text).trim().to_string();
        if content_without_mentions.is_empty() {
            return;
        }

        let msg = ChatMessage::new_user(final_text);
        self.known_message_ids.insert(msg.id.clone());
        let _ = append_message(&self.chat_file_path, &msg);
        self.last_chat_offset = std::fs::metadata(&self.chat_file_path).map(|m| m.len()).unwrap_or(self.last_chat_offset);
        self.chat_history.push(msg.clone());
        self.active_chain_depth = 0; // Reset chain depth on new user prompt
        self.chat_scroll_offset = 0;

        self.route_message(&msg);

        // Reset composer with default responder pre-filled
        self.reset_composer_with_default();
    }

    pub fn check_chat_log_updates(&mut self) {
        if let Ok((new_messages, new_offset)) =
            crate::chat::load_new_messages_from_offset(&self.chat_file_path, self.last_chat_offset)
        {
            self.last_chat_offset = new_offset;
            for msg in new_messages {
                if !self.known_message_ids.contains(&msg.id) {
                    self.known_message_ids.insert(msg.id.clone());
                    let is_new = !self.initial_message_ids.contains(&msg.id);
                    self.chat_history.push(msg.clone());

                    // Clear thinking state for msg.sender
                    self.active_tasks.remove(&msg.sender.to_lowercase());

                    if is_new {
                        self.route_message(&msg);
                    }
                }
            }
        }
    }

    pub fn route_message(&mut self, msg: &ChatMessage) {
        self.route_message_with_delay(msg, 0);
    }

    pub fn determine_target_clients(&mut self, msg: &ChatMessage) -> Vec<usize> {
        let mentions = extract_mentions(&msg.message);

        // Loop & echo prevention: Coordinator / System announcements without mentions do not trigger turns
        if (msg.sender.eq_ignore_ascii_case("Coordinator")
            || msg.sender.eq_ignore_ascii_case("System"))
            && mentions.is_empty()
        {
            return Vec::new();
        }

        let mut target_indices = Vec::new();

        if let Some(target_role_agent) = extract_role_instruction_target(&msg.message) {
            // Role instructions must ONLY be delivered to the appropriate role agent
            for (idx, client) in self.clients.iter().enumerate() {
                if client.name.eq_ignore_ascii_case(&target_role_agent)
                    && !client.name.eq_ignore_ascii_case(&msg.sender)
                {
                    target_indices.push(idx);
                    break;
                }
            }
        } else {
            let is_broadcast = mentions.iter().any(|m| m.eq_ignore_ascii_case("all"));

            if is_broadcast {
                for idx in 0..self.clients.len() {
                    // If message is from an agent, don't echo back to the same agent
                    if !self.clients[idx]
                        .name
                        .eq_ignore_ascii_case(&msg.sender)
                    {
                        target_indices.push(idx);
                    }
                }
            } else if !mentions.is_empty() {
                for mention in &mentions {
                    if mention.eq_ignore_ascii_case("User")
                        || mention.eq_ignore_ascii_case("Coordinator")
                        || mention.eq_ignore_ascii_case("System")
                    {
                        continue;
                    }
                    for (idx, client) in self.clients.iter().enumerate() {
                        if client.name.eq_ignore_ascii_case(mention)
                            && !client.name.eq_ignore_ascii_case(&msg.sender)
                            && !target_indices.contains(&idx)
                        {
                            target_indices.push(idx);
                            break;
                        }
                    }
                }
            }

            if target_indices.is_empty() && msg.sender == "User" {
                // Collaborative Fallback Mode: no agent targeted in user input
                if !self.clients.is_empty() {
                    let target = self.collaborative_next_idx % self.clients.len();
                    target_indices.push(target);
                    self.collaborative_next_idx = (self.collaborative_next_idx + 1) % self.clients.len();
                }
            }
        }

        target_indices
    }

    pub fn route_message_with_delay(&mut self, msg: &ChatMessage, base_delay_ms: u64) {
        // Chain depth limit check
        if self.active_chain_depth >= self.max_chain_depth {
            let warn_msg = ChatMessage::new_coordinator(
                format!(
                    "Chain depth limit reached ({}). Pausing autonomous agent turns.",
                    self.max_chain_depth
                ),
                Some(msg.id.clone()),
            );
            self.known_message_ids.insert(warn_msg.id.clone());
            let _ = append_message(&self.chat_file_path, &warn_msg);
            self.chat_history.push(warn_msg);
            return;
        }

        let target_indices = self.determine_target_clients(msg);

        if !target_indices.is_empty() {
            self.active_chain_depth += 1;
            let sanitized_content = strip_at_mentions(&msg.message);

            for (stagger_idx, &idx) in target_indices.iter().enumerate() {
                if let Some(client) = self.clients.get(idx) {
                    if !msg.sender.eq_ignore_ascii_case("Coordinator")
                        && !msg.sender.eq_ignore_ascii_case("System")
                    {
                        self.active_tasks.insert(
                            client.name.to_lowercase(),
                            ActiveTask {
                                agent_name: client.name.clone(),
                                agent_role: client.role.clone(),
                                task_ref: msg.id.clone(),
                                start_time: std::time::Instant::now(),
                            },
                        );
                    }
                    let task_prompt = format!(
                        "Task from {} ({}): {} [Ref: {}]",
                        msg.sender, msg.role, sanitized_content, msg.id
                    );
                    // Atomic composite prompt: prepend startup instructions (general + role) if client hasn't been initialized yet
                    let prompt = if !self.initialized_clients.contains(&idx) {
                        self.initialized_clients.insert(idx);
                        if let Some(client_cfg) = self.config.clients.get(idx) {
                            let inst = client_cfg.instructions.trim();
                            let mut vars = self.config.instruction_vars_for(client_cfg);
                            let chat_str = self.chat_file_path.display().to_string();
                            vars.insert("chat_file".to_string(), chat_str.clone());
                            vars.insert("chatfile".to_string(), chat_str);
                            let startup_instructions = build_startup_instructions_with_vars(
                                &self.config.general_instructions,
                                &client_cfg.name,
                                &client_cfg.role,
                                inst,
                                &vars,
                            );
                            format!("{}\n\n{}", startup_instructions, task_prompt)
                        } else {
                            task_prompt
                        }
                    } else {
                        task_prompt
                    };
                    let final_delay = base_delay_ms + (stagger_idx as u64 * 150);
                    client.simulate_keyboard_input_with_delay(&prompt, final_delay);
                }
            }
        }
    }

    pub fn shutdown(&mut self) {
        for client in &mut self.clients {
            client.terminate();
        }
    }
}

pub fn extract_screen_text(
    screen: &vt100::Screen,
    start: (u16, u16),
    end: (u16, u16),
) -> String {
    let (screen_rows, screen_cols) = screen.size();
    let max_row = screen_rows.get();
    let max_col = screen_cols.get();

    let (r1, c1, r2, c2) = if (start.1, start.0) <= (end.1, end.0) {
        (start.1, start.0, end.1, end.0)
    } else {
        (end.1, end.0, start.1, start.0)
    };

    let mut lines = Vec::new();
    for r in r1..=r2.min(max_row.saturating_sub(1)) {
        let col_start = if r == r1 { c1 } else { 0 };
        let col_end = if r == r2 { c2 } else { max_col.saturating_sub(1) };

        let mut line_buf = String::new();
        let mut skip_cols = 0;
        for c in col_start..=col_end.min(max_col.saturating_sub(1)) {
            if skip_cols > 0 {
                skip_cols -= 1;
                continue;
            }
            if let Some(cell) = screen.cell(r, c) {
                let s = cell.contents();
                if s.is_empty() {
                    line_buf.push(' ');
                } else {
                    line_buf.push_str(s);
                    let w = s
                        .chars()
                        .map(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1))
                        .sum::<usize>();
                    skip_cols = w.min(2).saturating_sub(1);
                }
            } else {
                line_buf.push(' ');
            }
        }
        lines.push(line_buf.trim_end().to_string());
    }

    lines.join("\n")
}

pub fn extract_grid_text(
    grid: &[Vec<String>],
    start: (u16, u16),
    end: (u16, u16),
) -> String {
    if grid.is_empty() {
        return String::new();
    }
    let max_row = grid.len() as u16;

    let (r1, c1, r2, c2) = if (start.1, start.0) <= (end.1, end.0) {
        (start.1, start.0, end.1, end.0)
    } else {
        (end.1, end.0, start.1, start.0)
    };

    let mut lines = Vec::new();
    for r in r1..=r2.min(max_row.saturating_sub(1)) {
        let row_cells = &grid[r as usize];
        let max_col = row_cells.len() as u16;
        if max_col == 0 {
            lines.push(String::new());
            continue;
        }
        let col_start = if r == r1 { c1 } else { 0 };
        let col_end = if r == r2 { c2 } else { max_col.saturating_sub(1) };

        let mut line_buf = String::new();
        let mut skip_cols = 0;
        for c in col_start..=col_end.min(max_col.saturating_sub(1)) {
            if skip_cols > 0 {
                skip_cols -= 1;
                continue;
            }
            let s = &row_cells[c as usize];
            line_buf.push_str(s);
            let w = s
                .chars()
                .map(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1))
                .sum::<usize>();
            skip_cols = w.min(2).saturating_sub(1);
        }
        lines.push(line_buf.trim_end().to_string());
    }

    lines.join("\n")
}

pub fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARSET[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARSET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARSET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARSET[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

pub fn copy_to_clipboard_osc52(text: &str) {
    if text.is_empty() {
        return;
    }
    use std::io::Write;
    let encoded = base64_encode(text.as_bytes());
    let osc52 = format!("\x1b]52;c;{}\x07", encoded);
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(osc52.as_bytes());
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_app_routing_and_fallback() {
        let tmp_dir = std::env::temp_dir().join(format!("test_mllm_app_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp_dir);

        let config = AppConfig {
            prefix_key: "alt".to_string(),
            general_instructions: "Instructions".to_string(),
            clients: vec![
                crate::config::ClientConfig {
                    number: 1,
                    name: "Architect".to_string(),
                    role: "Lead Architect".to_string(),
                    command: "cat".to_string(),
                    instructions: "Lead".to_string(),
                    extra_args: vec![],
                },
                crate::config::ClientConfig {
                    number: 2,
                    name: "Reviewer".to_string(),
                    role: "Reviewer".to_string(),
                    command: "cat".to_string(),
                    instructions: "Review".to_string(),
                    extra_args: vec![],
                },
                crate::config::ClientConfig {
                    number: 3,
                    name: "TechnicalWriter".to_string(),
                    role: "Technical Writer".to_string(),
                    command: "cat".to_string(),
                    instructions: "Docs".to_string(),
                    extra_args: vec![],
                },
            ],
        };

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = AppState::new(config, Some(&tmp_dir), tx).unwrap();

        // 1. Mention routing: @Reviewer at the beginning of the text
        let msg = ChatMessage::new_user("@Reviewer Please audit this".to_string());
        app.route_message(&msg);
        assert_eq!(app.active_chain_depth, 1);

        // 2. Broadcast routing: @all at the beginning of the text
        let msg_all = ChatMessage::new_user("@all Broadcast task".to_string());
        app.route_message(&msg_all);
        assert_eq!(app.active_chain_depth, 2);

        // 3. Mentions in the middle of the text are ignored, triggering collaborative fallback
        let msg_mid = ChatMessage::new_user("Please audit this @Reviewer".to_string());
        app.route_message(&msg_mid);
        assert_eq!(app.active_chain_depth, 3);
        assert_eq!(app.collaborative_next_idx, 1);

        // 4. Collaborative fallback (no mention)
        let msg_fallback = ChatMessage::new_user("No mention here".to_string());
        app.route_message(&msg_fallback);
        assert_eq!(app.active_chain_depth, 4);
        assert_eq!(app.collaborative_next_idx, 2);

        // 5. Loop prevention: Coordinator messages do not trigger turns
        let msg_coord = ChatMessage::new_coordinator("Status announcement".to_string(), None);
        app.route_message(&msg_coord);
        assert_eq!(app.active_chain_depth, 4); // Unchanged

        // 6. Role instruction routing: only sends to the appropriate role agent,
        // even if peer mentions or @all appear in the role instruction body.
        let role_inst = ChatMessage::new_coordinator(
            "@Architect [Role Instructions: Lead Architect]\nDirect security to @Reviewer and announce to @all"
                .to_string(),
            None,
        );
        let targets = app.determine_target_clients(&role_inst);
        assert_eq!(targets, vec![0]); // Client 0 is Architect; Reviewer (1) is NOT targeted

        let role_inst_rev = ChatMessage::new_coordinator(
            "@Reviewer [Role Assignment: Reviewer & Auditor]".to_string(),
            None,
        );
        let targets_rev = app.determine_target_clients(&role_inst_rev);
        assert_eq!(targets_rev, vec![1]); // Client 1 is Reviewer; Architect (0) is NOT targeted

        // Canonical TechnicalWriter routing (case-insensitive matching strictly from config name field)
        let msg_tw = ChatMessage::new_user("@TechnicalWriter Document the system".to_string());
        assert_eq!(app.determine_target_clients(&msg_tw), vec![2]);
        let msg_lower = ChatMessage::new_user("@technicalwriter Document the system".to_string());
        assert_eq!(app.determine_target_clients(&msg_lower), vec![2]);
        let msg_upper = ChatMessage::new_user("@TECHNICALWRITER Document the system".to_string());
        assert_eq!(app.determine_target_clients(&msg_upper), vec![2]);

        // Multiline directive bullets with mixed-case mentions
        let msg_multi = ChatMessage::new_user(
            "Audit summary:\n- To @technicalwriter: update the API reference\n- @REVIEWER: verify coverage".to_string(),
        );
        let targets_multi = app.determine_target_clients(&msg_multi);
        assert!(targets_multi.contains(&1) && targets_multi.contains(&2));

        // Role instruction header with canonical name
        let role_inst_canonical = ChatMessage::new_coordinator(
            "@technicalwriter [Role Instructions: Technical Writer]".to_string(),
            None,
        );
        assert_eq!(app.determine_target_clients(&role_inst_canonical), vec![2]);

        // Non-config aliases are NOT hardcoded and do not resolve directly
        let msg_unconfigured = ChatMessage::new_coordinator(
            "@TechWriter Update documentation".to_string(),
            None,
        );
        assert_eq!(app.determine_target_clients(&msg_unconfigured), Vec::<usize>::new());

        // Role instruction header with unconfigured alias resolves to zero targets, no panic
        let role_inst_unconfigured = ChatMessage::new_coordinator(
            "@TechWriter [Role Instructions: Technical Writer]".to_string(),
            None,
        );
        assert_eq!(
            app.determine_target_clients(&role_inst_unconfigured),
            Vec::<usize>::new()
        );

        // User message with only an unconfigured alias falls back collaboratively (no panic)
        app.collaborative_next_idx = 0;
        let msg_user_alias =
            ChatMessage::new_user("@TechWriter Update documentation".to_string());
        assert_eq!(app.determine_target_clients(&msg_user_alias), vec![0]);
        assert_eq!(app.collaborative_next_idx, 1);

        // Mixed mention: unconfigured alias mid-sentence ignored (per protocol),
        // leading configured peer routes, fallback not consumed
        app.collaborative_next_idx = 0;
        let msg_mixed = ChatMessage::new_user(
            "@Reviewer and @TechWriter please coordinate".to_string(),
        );
        assert_eq!(app.determine_target_clients(&msg_mixed), vec![1]);
        assert_eq!(app.collaborative_next_idx, 0);

        // 7. Chain depth limit at 100
        assert_eq!(app.max_chain_depth, 100);
        app.active_chain_depth = 100;
        let msg_limit = ChatMessage::new_user("@Reviewer Over limit".to_string());
        app.route_message(&msg_limit);
        assert_eq!(app.active_chain_depth, 100);
        let last = app.chat_history.last().unwrap();
        assert!(last.message.contains("Chain depth limit reached (100)"));

        app.shutdown();
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_mouse_selection_clamping_to_pane() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;
        use std::collections::{HashMap, HashSet};

        let mut app = AppState {
            config: AppConfig {
                prefix_key: "alt".to_string(),
                general_instructions: String::new(),
                clients: vec![],
            },
            parsed_prefix: parse_prefix_key("alt").unwrap(),
            active_pane: 0,
            prefix_active: false,
            input_focused: true,
            composer_input: String::new(),
            cursor_pos: 0,
            chat_history: vec![],
            known_message_ids: HashSet::new(),
            initial_message_ids: HashSet::new(),
            clients: vec![],
            active_chain_depth: 0,
            max_chain_depth: 100,
            chat_file_path: PathBuf::from("test.jsonl"),
            pty_dimensions: HashMap::new(),
            collaborative_next_idx: 0,
            tab_completion: None,
            initialized_clients: HashSet::new(),
            mouse_selection: None,
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: 0,
            coordinator_buffer: Vec::new(),
            clipboard: None,
            active_tasks: HashMap::new(),
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // Setup mock bounds: Pane 1 (left) and Pane 0 (center)
        // Outer rect for Pane 1: (0, 0, 40, 20), inner rect: (1, 1, 38, 18)
        // Outer rect for Pane 0: (40, 0, 40, 20), inner rect: (41, 1, 38, 18)
        app.pane_bounds.insert(1, (Rect::new(0, 0, 40, 20), Rect::new(1, 1, 38, 18)));
        app.pane_bounds.insert(0, (Rect::new(40, 0, 40, 20), Rect::new(41, 1, 38, 18)));

        // 1. Click inside Pane 1 at screen coordinate (10, 5)
        let click_pane1 = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_event(click_pane1);
        assert_eq!(app.active_pane, 1);
        assert_eq!(
            app.mouse_selection,
            Some(PaneSelection {
                pane_number: 1,
                start: (9, 4), // 10 - 1 = 9, 5 - 1 = 4
                end: (9, 4),
            })
        );

        // 2. Drag far outside Pane 1 (e.g. column 120, row 80, crossing into other panes)
        let drag_outside = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 120,
            row: 80,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_event(drag_outside);
        let sel = app.mouse_selection.as_ref().unwrap();
        assert_eq!(sel.pane_number, 1);
        // Clamped to inner max: width - 1 = 37, height - 1 = 17
        assert_eq!(sel.end, (37, 17));

        // 3. Drag to top-left beyond inner border (0, 0)
        let drag_top_left = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_event(drag_top_left);
        let sel = app.mouse_selection.as_ref().unwrap();
        assert_eq!(sel.end, (0, 0));

        // 4. Click inside Pane 0 at screen coordinate (50, 10)
        let click_pane0 = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 50,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_event(click_pane0);
        assert_eq!(app.active_pane, 0);
        assert_eq!(
            app.mouse_selection,
            Some(PaneSelection {
                pane_number: 0,
                start: (9, 9), // 50 - 41 = 9, 10 - 1 = 9
                end: (9, 9),
            })
        );

        // 5. Esc clears selection
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.mouse_selection.is_none());
    }

    #[test]
    fn test_base64_encode_and_screen_text_extraction() {
        // Base64 encoding tests
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"Hello World"), "SGVsbG8gV29ybGQ=");

        // VT100 screen text extraction tests
        use std::num::NonZeroU16;
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(24).unwrap(),
            NonZeroU16::new(80).unwrap(),
            0,
        );
        parser.process(b"Line 1 Hello\r\nLine 2 World");
        let screen = parser.screen();

        let extracted_single_line = extract_screen_text(screen, (0, 0), (11, 0));
        assert_eq!(extracted_single_line, "Line 1 Hello");

        let extracted_multi_line = extract_screen_text(screen, (0, 0), (11, 1));
        assert_eq!(extracted_multi_line, "Line 1 Hello\nLine 2 World");
    }

    #[test]
    fn test_cycle_responder_tab() {
        use crate::config::{parse_prefix_key, AppConfig, ClientConfig};
        use std::collections::{HashMap, HashSet};
        use std::path::PathBuf;

        let mut app = AppState {
            config: AppConfig {
                prefix_key: "alt".to_string(),
                general_instructions: String::new(),
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
            },
            parsed_prefix: parse_prefix_key("alt").unwrap(),
            active_pane: 0,
            prefix_active: false,
            input_focused: true,
            composer_input: "@Architect ".to_string(),
            cursor_pos: 11,
            chat_history: vec![],
            known_message_ids: HashSet::new(),
            initial_message_ids: HashSet::new(),
            clients: vec![],
            active_chain_depth: 0,
            max_chain_depth: 100,
            chat_file_path: PathBuf::from("test.jsonl"),
            pty_dimensions: HashMap::new(),
            collaborative_next_idx: 0,
            tab_completion: None,
            initialized_clients: HashSet::new(),
            mouse_selection: None,
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: 0,
            coordinator_buffer: Vec::new(),
            clipboard: None,
            active_tasks: HashMap::new(),
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // Populate mock clients list in app
        use tokio::sync::mpsc;
        let (tx, _rx) = mpsc::unbounded_channel();
        for cfg in &app.config.clients {
            app.clients.push(
                ClientPane::new(
                    cfg.number,
                    cfg.name.clone(),
                    cfg.role.clone(),
                    "cat",
                    &[],
                    Path::new("."),
                    80,
                    24,
                    tx.clone(),
                )
                .unwrap(),
            );
        }

        // 1. Initial @Architect -> Tab cycles to @Reviewer
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.composer_input, "@Reviewer ");

        // 2. Tab again -> cycles to @Researcher
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.composer_input, "@Researcher ");

        // 3. Tab again -> cycles to @all
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.composer_input, "@all ");

        // 4. Tab again -> wraps to @Architect
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.composer_input, "@Architect ");

        // 5. Shift+Tab (BackTab) -> reverse cycle back to @all
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.composer_input, "@all ");

        // 6. With existing message text: @all What is the plan? -> Tab changes to @Architect What is the plan?
        app.composer_input = "@all What is the plan?".to_string();
        app.cursor_pos = app.composer_input.len();
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.composer_input, "@Architect What is the plan?");

        // 7. Empty input: "" -> Tab adds @Architect
        app.composer_input = "".to_string();
        app.cursor_pos = 0;
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.composer_input, "@Architect ");

        app.shutdown();
    }

    #[test]
    fn test_pageup_pagedown_scrollback() {
        use crate::config::{parse_prefix_key, AppConfig};
        use std::collections::{HashMap, HashSet};
        use std::path::PathBuf;

        let mut app = AppState {
            config: AppConfig {
                prefix_key: "alt".to_string(),
                general_instructions: String::new(),
                clients: vec![],
            },
            parsed_prefix: parse_prefix_key("alt").unwrap(),
            active_pane: 0,
            prefix_active: false,
            input_focused: true,
            composer_input: String::new(),
            cursor_pos: 0,
            chat_history: vec![],
            known_message_ids: HashSet::new(),
            initial_message_ids: HashSet::new(),
            clients: vec![],
            active_chain_depth: 0,
            max_chain_depth: 100,
            chat_file_path: PathBuf::from("test.jsonl"),
            pty_dimensions: HashMap::new(),
            collaborative_next_idx: 0,
            tab_completion: None,
            initialized_clients: HashSet::new(),
            mouse_selection: None,
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: 0,
            coordinator_buffer: Vec::new(),
            clipboard: None,
            active_tasks: HashMap::new(),
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // 1. Pane 0 PageUp and PageDown
        app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.chat_scroll_offset, 10);

        app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.chat_scroll_offset, 20);

        app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.chat_scroll_offset, 10);

        // Esc clears scrollback
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.chat_scroll_offset, 0);

        // 2. Client Pane PageUp and PageDown
        use tokio::sync::mpsc;
        let (tx, _rx) = mpsc::unbounded_channel();
        let pane = ClientPane::new(1, "Architect".to_string(), "Architect".to_string(), "cat", &[], Path::new("."), 80, 24, tx).unwrap();
        // Feed 100 lines into the client pane to build scrollback
        if let Ok(mut parser) = pane.vt_parser.lock() {
            for i in 0..100 {
                parser.process(format!("Log line {}\r\n", i).as_bytes());
            }
        }
        app.clients.push(pane);
        app.active_pane = 1;

        app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        let offset = app.pane_scroll_offsets.get(&1).copied().unwrap_or(0);
        assert_eq!(offset, 10);

        app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        let offset = app.pane_scroll_offsets.get(&1).copied().unwrap_or(0);
        assert_eq!(offset, 0);

        app.shutdown();
    }

    #[test]
    fn test_extract_grid_text_and_wide_characters() {
        let grid = vec![
            vec!["H".to_string(), "e".to_string(), "l".to_string(), "l".to_string(), "o".to_string(), " ".to_string()],
            vec!["W".to_string(), "o".to_string(), "r".to_string(), "l".to_string(), "d".to_string(), "!".to_string()],
        ];

        // 1. Single-line extraction
        let text1 = extract_grid_text(&grid, (0, 0), (4, 0));
        assert_eq!(text1, "Hello");

        // 2. Multiline extraction
        let text2 = extract_grid_text(&grid, (0, 0), (4, 1));
        assert_eq!(text2, "Hello\nWorld");

        // 3. Reverse drag extraction (bottom right to top left)
        let text3 = extract_grid_text(&grid, (4, 1), (0, 0));
        assert_eq!(text3, "Hello\nWorld");

        // 4. Wide character extraction (with spacer cell skip)
        let wide_grid = vec![
            vec!["안".to_string(), " ".to_string(), "녕".to_string(), " ".to_string(), "!".to_string()],
        ];
        let wide_text = extract_grid_text(&wide_grid, (0, 0), (4, 0));
        assert_eq!(wide_text, "안녕!");

        // 5. ZWJ composite emoji (multi-codepoint, e.g. technologist 👨‍💻)
        let zwj_grid = vec![
            vec!["👨‍💻".to_string(), " ".to_string(), "O".to_string(), "K".to_string()],
        ];
        let zwj_text = extract_grid_text(&zwj_grid, (0, 0), (3, 0));
        assert_eq!(zwj_text, "👨‍💻OK");
    }

    #[test]
    fn test_ctrl_c_selection_copy_interception() {
        use std::collections::{HashMap, HashSet};
        let mut app = AppState {
            config: AppConfig {
                prefix_key: "alt".to_string(),
                general_instructions: String::new(),
                clients: vec![],
            },
            parsed_prefix: parse_prefix_key("alt").unwrap(),
            active_pane: 0,
            prefix_active: false,
            input_focused: true,
            composer_input: String::new(),
            cursor_pos: 0,
            chat_history: vec![],
            known_message_ids: HashSet::new(),
            initial_message_ids: HashSet::new(),
            clients: vec![],
            active_chain_depth: 0,
            max_chain_depth: 100,
            chat_file_path: PathBuf::from("test.jsonl"),
            pty_dimensions: HashMap::new(),
            collaborative_next_idx: 0,
            tab_completion: None,
            initialized_clients: HashSet::new(),
            mouse_selection: None,
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: 0,
            coordinator_buffer: vec![
                vec!["T".to_string(), "e".to_string(), "s".to_string(), "t".to_string()],
            ],
            clipboard: None,
            active_tasks: HashMap::new(),
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // 1. Without active selection, Ctrl+C triggers application quit (returns true)
        let quit_evt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.handle_key_event(quit_evt));

        // 2. With active selection, Ctrl+C copies selection, clears selection, and does NOT quit (returns false)
        app.mouse_selection = Some(PaneSelection {
            pane_number: 0,
            start: (0, 0),
            end: (3, 0),
        });

        assert!(!app.handle_key_event(quit_evt));
        assert!(app.mouse_selection.is_none());

        // 3. With whitespace/empty selection, Ctrl+C still clears selection and suppresses quit
        app.mouse_selection = Some(PaneSelection {
            pane_number: 0,
            start: (0, 0),
            end: (1, 0),
        });
        app.coordinator_buffer = vec![vec![" ".to_string(), " ".to_string()]];
        assert!(!app.handle_key_event(quit_evt));
        assert!(app.mouse_selection.is_none());
    }

    #[test]
    fn test_prefix_y_and_right_click_copy() {
        use std::collections::{HashMap, HashSet};
        let mut app = AppState {
            config: AppConfig {
                prefix_key: "alt".to_string(),
                general_instructions: String::new(),
                clients: vec![],
            },
            parsed_prefix: parse_prefix_key("alt").unwrap(),
            active_pane: 0,
            prefix_active: true,
            input_focused: true,
            composer_input: String::new(),
            cursor_pos: 0,
            chat_history: vec![],
            known_message_ids: HashSet::new(),
            initial_message_ids: HashSet::new(),
            clients: vec![],
            active_chain_depth: 0,
            max_chain_depth: 100,
            chat_file_path: PathBuf::from("test.jsonl"),
            pty_dimensions: HashMap::new(),
            collaborative_next_idx: 0,
            tab_completion: None,
            initialized_clients: HashSet::new(),
            mouse_selection: Some(PaneSelection {
                pane_number: 0,
                start: (0, 0),
                end: (3, 0),
            }),
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: 0,
            coordinator_buffer: vec![
                vec!["C".to_string(), "o".to_string(), "p".to_string(), "y".to_string()],
            ],
            clipboard: None,
            active_tasks: HashMap::new(),
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // Prefix + y copies selection and resets prefix_active
        let y_evt = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(!app.handle_key_event(y_evt));
        assert!(!app.prefix_active);

        // Right-click copies selection
        let right_click = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Right),
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse_event(right_click);
    }
}
