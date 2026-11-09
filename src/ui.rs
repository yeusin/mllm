use crate::app::{ActiveTask, AppState};
use crate::chat::{format_local_timestamp, ChatMessage};
use crate::rich_text::{display_width, render_rich_message_body, safe_truncate_id};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};
use ratatui::Frame;


pub fn calculate_layout(area: Rect, _client_count: usize) -> (Rect, Rect, Rect, Rect) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let workspace_area = root[0];
    let status_bar_area = root[1];

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(10, 32),
            Constraint::Ratio(12, 32),
            Constraint::Ratio(10, 32),
        ])
        .split(workspace_area);

    (columns[0], columns[1], columns[2], status_bar_area)
}

pub fn convert_vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn render(frame: &mut Frame, state: &mut AppState) {
    let size = frame.area();
    if size.width < 20 || size.height < 5 {
        return;
    }

    // 1. Root split: Workspace vs Bottom Status Bar
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(size);

    let workspace_area = root[0];
    let status_area = root[1];

    let client_count = state.clients.len();

    // 2. Symmetrical Column Partitioning
    if client_count == 0 {
        // Only Coordinator
        render_coordinator_pane(frame, state, workspace_area);
    } else if client_count == 1 {
        // N = 1: Left [Pane 1], Center [Pane 0]
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(workspace_area);

        render_client_pane(frame, state, 0, cols[0]);
        render_coordinator_pane(frame, state, cols[1]);
    } else {
        // N >= 2: Left Column, Center Column (1.2fr / ~38%), Right Column
        let (left_area, center_area, right_area, _) = calculate_layout(size, client_count);

        // Symmetrical partition: mid = client_count / 2
        let mid = client_count / 2;
        let left_count = mid;
        let right_count = client_count - mid;

        // Split left column vertically
        if left_count > 0 {
            let left_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints(vec![
                    Constraint::Ratio(1, left_count as u32);
                    left_count
                ])
                .split(left_area);

            for (i, area) in left_layout.iter().enumerate() {
                render_client_pane(frame, state, i, *area);
            }
        }

        // Center Coordinator
        render_coordinator_pane(frame, state, center_area);

        // Split right column vertically
        if right_count > 0 {
            let right_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints(vec![
                    Constraint::Ratio(1, right_count as u32);
                    right_count
                ])
                .split(right_area);

            for (i, area) in right_layout.iter().enumerate() {
                render_client_pane(frame, state, mid + i, *area);
            }
        }
    }

    // 3. Render Status Bar
    render_status_bar(frame, state, status_area);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerLayout {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

pub fn wrap_composer_text(
    input: &str,
    cursor_byte_pos: usize,
    width: usize,
) -> ComposerLayout {
    let prefix_len = 2; // "> " or "  "
    let content_width = width.saturating_sub(prefix_len).max(1);

    let safe_cursor_pos = if cursor_byte_pos <= input.len() {
        let mut p = cursor_byte_pos;
        while p > 0 && !input.is_char_boundary(p) {
            p -= 1;
        }
        p
    } else {
        input.len()
    };

    if input.is_empty() {
        return ComposerLayout {
            lines: vec!["> ".to_string()],
            cursor_line: 0,
            cursor_col: prefix_len,
        };
    }

    let mut visual_lines = Vec::new();
    let mut cursor_pos_found = None;
    let mut current_byte = 0;

    let logical_lines: Vec<&str> = input.split('\n').collect();
    let total_logical = logical_lines.len();

    for (logical_idx, &logical_line) in logical_lines.iter().enumerate() {
        let logical_start = current_byte;
        let logical_end = current_byte + logical_line.len();

        if cursor_pos_found.is_none() && safe_cursor_pos == logical_start {
            cursor_pos_found = Some((visual_lines.len(), prefix_len));
        }

        if logical_line.is_empty() {
            let prefix = if visual_lines.is_empty() { "> " } else { "  " };
            visual_lines.push(prefix.to_string());
            if cursor_pos_found.is_none() && safe_cursor_pos == logical_end {
                cursor_pos_found = Some((visual_lines.len() - 1, prefix_len));
            }
        } else {
            // Split logical line into words and whitespace segments
            let mut segments: Vec<&str> = Vec::new();
            let mut start = 0;
            let mut in_ws = false;
            for (idx, ch) in logical_line.char_indices() {
                let ws = ch.is_whitespace();
                if idx == 0 {
                    in_ws = ws;
                } else if ws != in_ws {
                    segments.push(&logical_line[start..idx]);
                    start = idx;
                    in_ws = ws;
                }
            }
            if start < logical_line.len() {
                segments.push(&logical_line[start..]);
            }

            let mut cur_line_text = String::new();
            let mut cur_line_width = 0;
            let mut seg_byte_offset = logical_start;

            for segment in segments {
                let seg_width = display_width(segment);
                let is_ws = segment.chars().all(char::is_whitespace);

                // If adding a word exceeds content_width, wrap to new line
                if !is_ws && cur_line_width > 0 && cur_line_width + seg_width > content_width {
                    let prefix = if visual_lines.is_empty() { "> " } else { "  " };
                    visual_lines.push(format!("{}{}", prefix, cur_line_text));
                    cur_line_text.clear();
                    cur_line_width = 0;
                }

                // If word is longer than content_width on its own, break word
                if !is_ws && seg_width > content_width {
                    for ch in segment.chars() {
                        let ch_bytes = ch.len_utf8();
                        let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                        if cursor_pos_found.is_none() && safe_cursor_pos == seg_byte_offset {
                            cursor_pos_found = Some((visual_lines.len(), prefix_len + cur_line_width));
                        }
                        if cur_line_width + ch_w > content_width && cur_line_width > 0 {
                            let prefix = if visual_lines.is_empty() { "> " } else { "  " };
                            visual_lines.push(format!("{}{}", prefix, cur_line_text));
                            cur_line_text.clear();
                            cur_line_width = 0;
                        }
                        cur_line_text.push(ch);
                        cur_line_width += ch_w;
                        seg_byte_offset += ch_bytes;
                    }
                    if cursor_pos_found.is_none() && safe_cursor_pos == seg_byte_offset {
                        cursor_pos_found = Some((visual_lines.len(), prefix_len + cur_line_width));
                    }
                    continue;
                }

                // For normal words and whitespace
                if is_ws && cur_line_width >= content_width {
                    let prefix = if visual_lines.is_empty() { "> " } else { "  " };
                    visual_lines.push(format!("{}{}", prefix, cur_line_text));
                    cur_line_text.clear();
                    cur_line_width = 0;
                }

                for ch in segment.chars() {
                    let ch_bytes = ch.len_utf8();
                    let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                    if cursor_pos_found.is_none() && safe_cursor_pos == seg_byte_offset {
                        cursor_pos_found = Some((visual_lines.len(), prefix_len + cur_line_width));
                    }
                    if cur_line_width + ch_w > content_width && cur_line_width > 0 {
                        let prefix = if visual_lines.is_empty() { "> " } else { "  " };
                        visual_lines.push(format!("{}{}", prefix, cur_line_text));
                        cur_line_text.clear();
                        cur_line_width = 0;
                    }
                    cur_line_text.push(ch);
                    cur_line_width += ch_w;
                    seg_byte_offset += ch_bytes;
                }
            }

            let prefix = if visual_lines.is_empty() { "> " } else { "  " };
            visual_lines.push(format!("{}{}", prefix, cur_line_text));
        }

        current_byte = logical_end;
        if logical_idx + 1 < total_logical {
            if cursor_pos_found.is_none() && safe_cursor_pos == current_byte {
                let last_line_idx = visual_lines.len().saturating_sub(1);
                let last_col = display_width(&visual_lines[last_line_idx]);
                cursor_pos_found = Some((last_line_idx, last_col));
            }
            current_byte += 1; // +1 for '\n'
        } else if cursor_pos_found.is_none() && safe_cursor_pos >= current_byte {
            let last_line_idx = visual_lines.len().saturating_sub(1);
            let last_col = display_width(&visual_lines[last_line_idx]);
            cursor_pos_found = Some((last_line_idx, last_col));
        }
    }

    let (mut c_line, mut c_col) = cursor_pos_found.unwrap_or_else(|| {
        let last_line_idx = visual_lines.len().saturating_sub(1);
        let last_col = display_width(&visual_lines[last_line_idx]);
        (last_line_idx, last_col)
    });

    if width > prefix_len && c_col >= width {
        c_line += 1;
        c_col = prefix_len;
        if c_line >= visual_lines.len() {
            visual_lines.push("  ".to_string());
        }
    }

    ComposerLayout {
        lines: visual_lines,
        cursor_line: c_line,
        cursor_col: c_col,
    }
}

pub fn sender_palette(sender: &str) -> (Color, &'static str) {
    match sender {
        "User" => (Color::Green, "●"),
        "Coordinator" => (Color::Magenta, "⚡"),
        "Architect" => (Color::Cyan, "◆"),
        "Reviewer" => (Color::Yellow, "▲"),
        "Researcher" => (Color::LightBlue, "■"),
        "TechnicalWriter" | "Copywriter" => (Color::LightRed, "✦"),
        "Designer" => (Color::LightMagenta, "★"),
        "Marketer" => (Color::LightGreen, "⯈"),
        _ => {
            const PALETTE: [(Color, &'static str); 6] = [
                (Color::Cyan, "◆"),
                (Color::Yellow, "▲"),
                (Color::LightBlue, "■"),
                (Color::LightRed, "✦"),
                (Color::LightMagenta, "★"),
                (Color::LightGreen, "⯈"),
            ];
            let hash = sender.bytes().fold(0usize, |acc, b| acc.wrapping_add(b as usize));
            PALETTE[hash % PALETTE.len()]
        }
    }
}

pub fn format_chat_message(
    msg: &ChatMessage,
    chat_history: &[ChatMessage],
    available_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let (sender_color, badge) = sender_palette(&msg.sender);
    let short_id = safe_truncate_id(&msg.id, 4);
    let time_str = format_local_timestamp(msg.timestamp);

    // 1. Header Card Line with high-contrast role pill badge & card rule
    let sender_text = format!(" {badge} {} ", msg.sender);
    let role_text = format!("[{}]", msg.role);
    let meta_text = if msg.superseded_by.is_some() {
        format!("#{short_id} {time_str} [SUPERSEDED]")
    } else if msg.fallback == Some(true) {
        format!("#{short_id} {time_str} [FALLBACK]")
    } else {
        format!("#{short_id} {time_str}")
    };

    let mut header_spans = Vec::new();
    header_spans.push(Span::styled("─── ", Style::default().fg(Color::DarkGray)));

    let pill_fg = if sender_color == Color::Magenta {
        Color::White
    } else {
        Color::Black
    };
    header_spans.push(Span::styled(
        sender_text.clone(),
        Style::default()
            .fg(pill_fg)
            .bg(sender_color)
            .add_modifier(Modifier::BOLD),
    ));

    // Calculate length of components to adaptively fit available width without wrapping
    let base_len = 4 + sender_text.chars().count() + 1 + meta_text.chars().count();
    let with_role_len = base_len + 1 + role_text.chars().count();

    let include_role = available_width >= with_role_len + 4;
    let used_len = if include_role {
        header_spans.push(Span::styled(" ", Style::default()));
        header_spans.push(Span::styled(
            role_text,
            Style::default().fg(Color::DarkGray),
        ));
        header_spans.push(Span::styled(" ", Style::default()));
        with_role_len + 1
    } else {
        header_spans.push(Span::styled(" ", Style::default()));
        base_len + 1
    };

    header_spans.push(Span::styled(
        meta_text,
        Style::default().fg(Color::DarkGray),
    ));

    if available_width > used_len + 2 {
        let pad_len = available_width - used_len - 1;
        header_spans.push(Span::styled(" ", Style::default()));
        header_spans.push(Span::styled(
            "─".repeat(pad_len),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::from(header_spans));

    // 2. Thread Context Indicator
    if let Some(ref reply_id) = msg.reply_to {
        let short_reply = safe_truncate_id(reply_id, 4);

        let target_sender = chat_history
            .iter()
            .find(|m| m.id == *reply_id)
            .map(|m| m.sender.as_str());

        let mut reply_spans = vec![
            Span::styled("  ↳ ", Style::default().fg(sender_color)),
            Span::styled("in reply to ", Style::default().fg(Color::DarkGray)),
        ];

        if let Some(parent_sender) = target_sender {
            let (parent_color, _) = sender_palette(parent_sender);
            reply_spans.push(Span::styled(
                format!("@{parent_sender} "),
                Style::default().fg(parent_color).add_modifier(Modifier::BOLD),
            ));
        }

        reply_spans.push(Span::styled(
            format!("(#{short_reply})"),
            Style::default().fg(Color::DarkGray),
        ));

        lines.push(Line::from(reply_spans));
    }

    // 3. Message Body with Rich Formatting (Markdown, ANSI, Emojis, Mentions)
    let body_lines = render_rich_message_body(&msg.message, available_width, sender_palette);
    lines.extend(body_lines);

    lines
}


pub fn format_thinking_card(
    task: &ActiveTask,
    chat_history: &[ChatMessage],
    available_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let (sender_color, badge) = sender_palette(&task.agent_name);
    let short_id = safe_truncate_id(&task.task_ref, 4);

    let elapsed = task.start_time.elapsed();
    let elapsed_secs = elapsed.as_secs_f32();

    const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let spinner_idx = ((elapsed.as_millis() / 100) as usize) % SPINNER_FRAMES.len();
    let spinner = SPINNER_FRAMES[spinner_idx];

    let sender_text = format!(" {badge} {} ", task.agent_name);
    let role_text = format!("[{}]", task.agent_role);
    let status_text = format!("{spinner} THINKING ({:>4.1}s)", elapsed_secs);

    let mut header_spans = Vec::new();
    header_spans.push(Span::styled("─── ", Style::default().fg(Color::DarkGray)));

    let pill_fg = if sender_color == Color::Magenta {
        Color::White
    } else {
        Color::Black
    };
    header_spans.push(Span::styled(
        sender_text.clone(),
        Style::default()
            .fg(pill_fg)
            .bg(sender_color)
            .add_modifier(Modifier::BOLD),
    ));

    let base_len = 4 + sender_text.chars().count() + 1 + status_text.chars().count();
    let with_role_len = base_len + 1 + role_text.chars().count();

    let include_role = available_width >= with_role_len + 4;
    let used_len = if include_role {
        header_spans.push(Span::styled(" ", Style::default()));
        header_spans.push(Span::styled(
            role_text,
            Style::default().fg(Color::DarkGray),
        ));
        header_spans.push(Span::styled(" ", Style::default()));
        with_role_len + 1
    } else {
        header_spans.push(Span::styled(" ", Style::default()));
        base_len + 1
    };

    header_spans.push(Span::styled(
        status_text,
        Style::default()
            .fg(sender_color)
            .add_modifier(Modifier::BOLD),
    ));

    if available_width > used_len + 2 {
        let pad_len = available_width - used_len - 1;
        header_spans.push(Span::styled(" ", Style::default()));
        header_spans.push(Span::styled(
            "─".repeat(pad_len),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::from(header_spans));

    // Thread Context Indicator: in reply to parent task
    if !task.task_ref.is_empty() {
        let target_sender = chat_history
            .iter()
            .find(|m| m.id == task.task_ref)
            .map(|m| m.sender.as_str());

        let mut reply_spans = vec![
            Span::styled("  ↳ ", Style::default().fg(sender_color)),
            Span::styled("processing task from ", Style::default().fg(Color::DarkGray)),
        ];

        if let Some(parent_sender) = target_sender {
            let (parent_color, _) = sender_palette(parent_sender);
            reply_spans.push(Span::styled(
                format!("@{parent_sender} "),
                Style::default().fg(parent_color).add_modifier(Modifier::BOLD),
            ));
        }

        reply_spans.push(Span::styled(
            format!("(#{short_id})"),
            Style::default().fg(Color::DarkGray),
        ));

        lines.push(Line::from(reply_spans));
    }

    // Body Line: Pulsing message
    let body_spans = vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("{spinner} Generating response..."),
            Style::default()
                .fg(Color::Rgb(200, 200, 200))
                .add_modifier(Modifier::ITALIC),
        ),
    ];
    lines.push(Line::from(body_spans));

    lines
}

pub fn render_coordinator_pane(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let is_focused = state.active_pane == 0;
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let scroll_badge = if state.chat_scroll_offset > 0 {
        format!(" [SCROLLBACK: +{} lines] ", state.chat_scroll_offset)
    } else {
        String::new()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Pane 0: Coordinator Chat [Role: Coordinator]{} ", scroll_badge))
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    state.pane_bounds.insert(0, (area, inner));
    frame.render_widget(block, area);

    if inner.height < 4 {
        return;
    }

    // Split inner into chat history and composer input with adaptive height for wrapped & multi-line input
    let input_width = inner.width.saturating_sub(2) as usize;
    let layout = wrap_composer_text(&state.composer_input, state.cursor_pos, input_width);
    let max_composer = inner.height.saturating_sub(4).min(8).max(3);
    let composer_height = (layout.lines.len() as u16 + 2).clamp(3, max_composer);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(composer_height)])
        .split(inner);

    let chat_area = chunks[0];
    let composer_area = chunks[1];

    // Format chat messages with upgraded design: distinct role pill badges, card borders,
    // thread reply linking, rich markdown/ANSI/emoji formatting, parse-once cache,
    // and viewport-windowed rendering (R4.1 / R4.3)
    let available_width = chat_area.width as usize;

    // Prune exited or stale tasks
    for client in &mut state.clients {
        if client.check_status().is_some() {
            state.active_tasks.remove(&client.name.to_lowercase());
        }
    }
    state.active_tasks.retain(|_, task| task.start_time.elapsed().as_secs() < 600);

    // R4.1: Parse-once per-message cache & wrapped height calculation.
    // Recomputed ONLY when terminal width changes (resize) or new messages are appended.
    if state.chat_cache_width != available_width || state.message_heights.len() > state.chat_history.len() {
        state.chat_cache_width = available_width;
        state.render_cache.clear();
        state.message_heights.clear();
        state.chat_total_lines = 0;

        for msg in &state.chat_history {
            let formatted = format_chat_message(msg, &state.chat_history, available_width);
            let h = Paragraph::new(formatted.clone())
                .wrap(Wrap { trim: false })
                .line_count(chat_area.width) as usize + 1; // +1 for message separator Line::from("")
            state.message_heights.push(h);
            state.chat_total_lines += h;
            state.render_cache.insert(msg.id.clone(), (available_width, formatted));
        }
    } else if state.message_heights.len() < state.chat_history.len() {
        let start_idx = state.message_heights.len();
        for msg in &state.chat_history[start_idx..] {
            let formatted = format_chat_message(msg, &state.chat_history, available_width);
            let h = Paragraph::new(formatted.clone())
                .wrap(Wrap { trim: false })
                .line_count(chat_area.width) as usize + 1;
            state.message_heights.push(h);
            state.chat_total_lines += h;
            state.render_cache.insert(msg.id.clone(), (available_width, formatted));
        }
    }

    // Format active thinking cards
    let mut active_list: Vec<ActiveTask> = state.active_tasks.values().cloned().collect();
    active_list.sort_by_key(|t| t.start_time);

    let mut active_card_entries: Vec<(Vec<Line<'static>>, usize)> = Vec::with_capacity(active_list.len());
    let mut active_tasks_total = 0;
    for task in &active_list {
        let card_lines = format_thinking_card(task, &state.chat_history, available_width);
        let h = Paragraph::new(card_lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(chat_area.width) as usize + 1;
        active_card_entries.push((card_lines, h));
        active_tasks_total += h;
    }

    // Cached total lines: sum of pre-wrapped message heights + active tasks (kills per-frame full re-wrap!)
    let total_lines = state.chat_total_lines + active_tasks_total;
    let visible_lines = chat_area.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_lines);
    let clamped_offset = state.chat_scroll_offset.min(max_scroll);
    let scroll_y = max_scroll.saturating_sub(clamped_offset);
    let scroll_end = scroll_y + visible_lines;

    // Viewport-windowed rendering: materialize Vec<Line> ONLY for visible messages (R4.1 / R4.3)
    let mut text_lines = Vec::new();
    let mut current_y = 0;
    let mut first_visible_y = None;

    for (idx, msg) in state.chat_history.iter().enumerate() {
        let h = state.message_heights.get(idx).copied().unwrap_or(1);
        let next_y = current_y + h;
        if next_y > scroll_y && current_y < scroll_end {
            if first_visible_y.is_none() {
                first_visible_y = Some(current_y);
            }
            let msg_lines = if let Some((cached_width, cached_lines)) = state.render_cache.get(&msg.id) {
                if *cached_width == available_width {
                    cached_lines.clone()
                } else {
                    let formatted = format_chat_message(msg, &state.chat_history, available_width);
                    state.render_cache.insert(msg.id.clone(), (available_width, formatted.clone()));
                    formatted
                }
            } else {
                let formatted = format_chat_message(msg, &state.chat_history, available_width);
                state.render_cache.insert(msg.id.clone(), (available_width, formatted.clone()));
                formatted
            };
            text_lines.extend(msg_lines);
            text_lines.push(Line::from(""));
        }
        current_y = next_y;
        if current_y >= scroll_end {
            break;
        }
    }

    if current_y < scroll_end {
        for (card_lines, h) in active_card_entries {
            let next_y = current_y + h;
            if next_y > scroll_y && current_y < scroll_end {
                if first_visible_y.is_none() {
                    first_visible_y = Some(current_y);
                }
                text_lines.extend(card_lines);
                text_lines.push(Line::from(""));
            }
            current_y = next_y;
            if current_y >= scroll_end {
                break;
            }
        }
    }

    // Bounded memory cache: evict old entries if cache exceeds 2,000 entries (R4.3)
    // Because off-screen messages are not materialized during render, eviction does not thrash!
    if state.render_cache.len() > 2000 {
        let recent_ids: std::collections::HashSet<&str> = state
            .chat_history
            .iter()
            .rev()
            .take(1500)
            .map(|m| m.id.as_str())
            .collect();
        state.render_cache.retain(|k, _| recent_ids.contains(k.as_str()));
    }

    // Scroll offset into the visible slice
    let first_y = first_visible_y.unwrap_or(scroll_y);
    let viewport_scroll_y = scroll_y.saturating_sub(first_y);

    let paragraph = Paragraph::new(text_lines)
        .wrap(Wrap { trim: false })
        .scroll((viewport_scroll_y as u16, 0));
    frame.render_widget(paragraph, chat_area);

    // Target responder display indicator
    let (targets, is_default) = state.composer_targets();
    let target_label = if let Some(ref completion) = state.tab_completion {
        let list: Vec<String> = completion
            .matches
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if i == completion.current_idx {
                    format!("*@{}*", m)
                } else {
                    format!("@{}", m)
                }
            })
            .collect();
        format!(" [Tab: {}]", list.join(" | "))
    } else if !targets.is_empty() {
        let list: Vec<String> = targets.iter().map(|t| format!("@{}", t)).collect();
        if is_default {
            format!(" [Responder: {} (Default)]", list.join(", "))
        } else {
            format!(" [Responder: {}]", list.join(", "))
        }
    } else {
        String::new()
    };

    // Composer Input Widget
    let input_title = if is_focused && state.input_focused {
        format!(" Composer Input{} (Enter: Send, Shift+Enter: Newline) ", target_label)
    } else if is_focused {
        format!(" Composer Input{} (Press 'i' or Enter to Type) ", target_label)
    } else {
        format!(" Composer Input{} ", target_label)
    };

    let input_border_color = if is_focused && state.input_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(input_title)
        .border_style(Style::default().fg(input_border_color));

    let input_inner = input_block.inner(composer_area);
    frame.render_widget(input_block, composer_area);

    let visible_lines = input_inner.height as usize;
    let scroll_y = if visible_lines > 0 && layout.cursor_line >= visible_lines {
        layout.cursor_line - visible_lines + 1
    } else {
        0
    };

    let composer_lines: Vec<Line> = layout.lines.into_iter().map(Line::from).collect();
    let input_para = Paragraph::new(composer_lines).scroll((scroll_y as u16, 0));
    frame.render_widget(input_para, input_inner);

    if is_focused && state.input_focused {
        let screen_y = input_inner.y + (layout.cursor_line.saturating_sub(scroll_y)) as u16;
        let screen_x = input_inner.x + layout.cursor_col as u16;
        if screen_x < input_inner.x + input_inner.width && screen_y < input_inner.y + input_inner.height {
            frame.set_cursor_position(Position::new(screen_x, screen_y));
        }
    }

    // Capture rendered inner grid into state.coordinator_buffer before applying selection styling
    if inner.width > 0 && inner.height > 0 {
        let mut grid = Vec::with_capacity(inner.height as usize);
        let buf = frame.buffer_mut();
        for r in 0..inner.height {
            let mut row = Vec::with_capacity(inner.width as usize);
            for c in 0..inner.width {
                let x = inner.x + c;
                let y = inner.y + r;
                let symbol = buf
                    .cell((x, y))
                    .map_or_else(|| " ".to_string(), |cell| cell.symbol().to_string());
                row.push(symbol);
            }
            grid.push(row);
        }
        state.coordinator_buffer = grid;
    }

    // Apply visual selection highlight if Pane 0 has an active mouse selection
    if let Some(sel) = &state.mouse_selection {
        if sel.pane_number == 0 && sel.start != sel.end && inner.width > 0 && inner.height > 0 {
            let (r1, c1, r2, c2) = if (sel.start.1, sel.start.0) <= (sel.end.1, sel.end.0) {
                (sel.start.1, sel.start.0, sel.end.1, sel.end.0)
            } else {
                (sel.end.1, sel.end.0, sel.start.1, sel.start.0)
            };

            let buf = frame.buffer_mut();
            for r in r1..=r2.min(inner.height.saturating_sub(1)) {
                let col_start = if r == r1 { c1 } else { 0 };
                let col_end = if r == r2 { c2 } else { inner.width.saturating_sub(1) };
                for c in col_start..=col_end.min(inner.width.saturating_sub(1)) {
                    let x = inner.x + c;
                    let y = inner.y + r;
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        let mut style = cell.style();
                        style = style.bg(Color::Rgb(50, 70, 100)).fg(Color::White);
                        cell.set_style(style);
                    }
                }
            }
        }
    }
}

fn render_client_pane(frame: &mut Frame, state: &mut AppState, client_idx: usize, area: Rect) {
    if client_idx >= state.clients.len() {
        return;
    }

    let default_block = Block::default().borders(Borders::ALL);
    let inner = default_block.inner(area);

    if inner.width > 0 && inner.height > 0 {
        // Synchronize PTY size to inner dimensions
        state.sync_pty_size(client_idx, inner.width, inner.height);
    }

    let exit_status = state.clients[client_idx].check_status();
    let client = &state.clients[client_idx];
    state.pane_bounds.insert(client.number, (area, inner));
    let is_focused = state.active_pane == client.number;

    let border_color = if is_focused {
        if exit_status.is_some() {
            Color::Red
        } else {
            Color::Cyan
        }
    } else if exit_status.is_some() {
        Color::Indexed(88)
    } else {
        Color::DarkGray
    };

    let is_busy = client.is_busy.load(std::sync::atomic::Ordering::Relaxed);
    let scroll_offset = state.pane_scroll_offsets.get(&client.number).copied().unwrap_or(0);
    let scroll_badge = if scroll_offset > 0 {
        format!(" [SCROLLBACK: -{} lines] ", scroll_offset)
    } else {
        String::new()
    };

    let status_badge = if let Some(code) = exit_status {
        format!(" [EXITED: {}]", code)
    } else if is_busy {
        " [TYPING...]".to_string()
    } else {
        String::new()
    };

    let title = format!(
        " Pane {}: {} [Role: {}]{}{} ",
        client.number, client.name, client.role, scroll_badge, status_badge
    );

    let block = default_block
        .title(title)
        .border_style(Style::default().fg(border_color));

    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let selection_range = if let Some(sel) = &state.mouse_selection {
        if sel.pane_number == client.number && sel.start != sel.end {
            Some((sel.start, sel.end))
        } else {
            None
        }
    } else {
        None
    };

    // Render VT100 Screen
    let (cursor_pos, show_cursor) = if let Ok(parser) = client.vt_parser.lock() {
        let screen = parser.screen();
        let (screen_rows, screen_cols) = screen.size();
        let (c_row, c_col) = screen.cursor_position();
        let hide_cursor = screen.hide_cursor();

        let term_widget = PtyTerminalWidget {
            screen,
            rows: screen_rows.get(),
            cols: screen_cols.get(),
            selection: selection_range,
        };
        frame.render_widget(term_widget, inner);

        ((c_col, c_row), !hide_cursor)
    } else {
        ((0, 0), false)
    };

    // If client pane is focused and cursor is visible, position the cursor
    if is_focused && show_cursor {
        let cursor_x = inner.x + cursor_pos.0;
        let cursor_y = inner.y + cursor_pos.1;
        if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }
}

struct PtyTerminalWidget<'a> {
    screen: &'a vt100::Screen,
    rows: u16,
    cols: u16,
    selection: Option<((u16, u16), (u16, u16))>,
}

impl<'a> Widget for PtyTerminalWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let max_r = area.height.min(self.rows);
        let max_c = area.width.min(self.cols);

        for row in 0..max_r {
            for col in 0..max_c {
                if let Some(cell) = self.screen.cell(row, col) {
                    let contents = cell.contents();
                    let fg = convert_vt_color(cell.fgcolor());
                    let bg = convert_vt_color(cell.bgcolor());

                    let mut style = Style::default().fg(fg).bg(bg);
                    if cell.bold() {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    if cell.italic() {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    if cell.underline() {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }

                    let is_selected = if let Some(((start_col, start_row), (end_col, end_row))) = self.selection {
                        let (r1, c1, r2, c2) = if (start_row, start_col) <= (end_row, end_col) {
                            (start_row, start_col, end_row, end_col)
                        } else {
                            (end_row, end_col, start_row, start_col)
                        };

                        if row < r1 || row > r2 {
                            false
                        } else if r1 == r2 {
                            col >= c1 && col <= c2
                        } else if row == r1 {
                            col >= c1
                        } else if row == r2 {
                            col <= c2
                        } else {
                            true
                        }
                    } else {
                        false
                    };

                    if is_selected || cell.inverse() {
                        style = style.add_modifier(Modifier::REVERSED);
                    }

                    let x = area.x + col;
                    let y = area.y + row;
                    if x < area.right() && y < area.bottom() {
                        let target_cell = &mut buf[(x, y)];
                        if !contents.is_empty() {
                            target_cell.set_symbol(contents);
                        } else {
                            target_cell.set_symbol(" ");
                        }
                        target_cell.set_style(style);
                    }
                }
            }
        }
    }
}

fn render_status_bar(frame: &mut Frame, state: &AppState, area: Rect) {
    let mut spans = Vec::new();

    // Session identifier
    spans.push(Span::styled(
        "[multi-llm] ",
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ));

    // Pane 0: Coordinator
    let p0_active = state.active_pane == 0;
    let p0_label = if p0_active {
        "0:Coordinator* "
    } else {
        "0:Coordinator "
    };
    let p0_style = if p0_active {
        Style::default()
            .fg(Color::White)
            .bg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Black).bg(Color::Green)
    };
    spans.push(Span::styled(p0_label, p0_style));

    // Client pane tabs
    for client in &state.clients {
        let is_active = state.active_pane == client.number;
        let label = if is_active {
            format!("{}:{}* ", client.number, client.name)
        } else {
            format!("{}:{} ", client.number, client.name)
        };
        let style = if is_active {
            Style::default()
                .fg(Color::White)
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Black).bg(Color::Green)
        };
        spans.push(Span::styled(label, style));
    }

    // Space filler
    spans.push(Span::styled(
        " ",
        Style::default().fg(Color::Black).bg(Color::Green),
    ));

    // Prefix indicator or disarmed label
    if state.prefix_active {
        spans.push(Span::styled(
            " [PREFIX] ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        let prefix_display = state.config.prefix_key.to_uppercase();
        spans.push(Span::styled(
            format!("[Prefix: {}] ", prefix_display),
            Style::default().fg(Color::Black).bg(Color::Green),
        ));
    }

    // Mode: COLLABORATE vs DIRECT
    let mode_str = if state.active_pane == 0 {
        "[COLLABORATE]"
    } else {
        "[DIRECT]"
    };
    spans.push(Span::styled(
        mode_str,
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ));

    let bar = Paragraph::new(Line::from(spans))
        .style(Style::default().bg(Color::Green));
    frame.render_widget(bar, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{parse_prefix_key, AppConfig};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;

    #[test]
    fn test_calculate_layout() {
        let area = Rect::new(0, 0, 160, 50);
        let (left, center, right, status) = calculate_layout(area, 3);

        assert_eq!(status.height, 1);
        assert_eq!(status.y, 49);
        assert_eq!(left.y, 0);
        assert_eq!(center.y, 0);
        assert_eq!(right.y, 0);
        assert_eq!(left.height, 49);
        assert_eq!(center.height, 49);
        assert_eq!(right.height, 49);

        // Center column is larger (12/32 vs 10/32)
        assert!(center.width >= left.width);
        assert!(center.width >= right.width);
    }

    #[test]
    fn test_coordinator_pane_auto_scrolls_to_latest_message() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use crate::chat::ChatMessage;
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
            chat_history: vec![
                ChatMessage {
                    id: "msg1".to_string(),
                    reply_to: None,
                    sender: "User".to_string(),
                    role: "Operator".to_string(),
                    message: "A ".repeat(200),
                    timestamp: 1000,
                    fallback: None,
                    superseded_by: None,
                },
                ChatMessage {
                    id: "msg2".to_string(),
                    reply_to: None,
                    sender: "Coordinator".to_string(),
                    role: "Coordinator".to_string(),
                    message: "B ".repeat(200),
                    timestamp: 1001,
                    fallback: None,
                    superseded_by: None,
                },
                ChatMessage {
                    id: "msg3".to_string(),
                    reply_to: None,
                    sender: "Architect".to_string(),
                    role: "Lead Architect".to_string(),
                    message: "FINAL_LATEST_MESSAGE".to_string(),
                    timestamp: 1002,
                    fallback: None,
                    superseded_by: None,
                },
            ],
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
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        let backend = TestBackend::new(50, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| {
            let area = Rect::new(0, 0, 50, 15);
            render_coordinator_pane(f, &mut app, area);
        }).unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("FINAL_LATEST_MESSAGE"), "Coordinator pane must show the latest chat message");
    }

    #[test]
    fn test_render_coordinator_multiline_composer() {
        use crate::config::{parse_prefix_key, AppConfig};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
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
            composer_input: "@Architect\nFirst multi-line\nSecond multi-line".to_string(),
            cursor_pos: 20,
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
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 120, 20);
                render_coordinator_pane(f, &mut app, area);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("Shift+Enter: Newline"));
        assert!(content.contains("First multi-line"));
        assert!(content.contains("Second multi-line"));
    }

    #[test]
    fn test_wrap_composer_text_long_line() {
        let text = "This is a very long text that must wrap nicely across multiple lines in the input bar.";
        let layout = wrap_composer_text(text, text.len(), 30);
        assert!(layout.lines.len() >= 3, "Expected at least 3 lines, got: {:?}", layout.lines);
        for line in &layout.lines {
            assert!(line.chars().count() <= 30, "Line exceeds width 30: {:?}", line);
        }
        assert_eq!(layout.cursor_line, layout.lines.len() - 1);
        assert!(layout.cursor_col <= 30);

        // Test cursor tracking at start
        let layout_start = wrap_composer_text(text, 0, 30);
        assert_eq!(layout_start.cursor_line, 0);
        assert_eq!(layout_start.cursor_col, 2); // after "> "
    }

    #[test]
    fn test_render_coordinator_long_text_wrapping() {
        use crate::config::{parse_prefix_key, AppConfig};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use std::collections::{HashMap, HashSet};
        use std::path::PathBuf;

        let long_input = "@Architect Here is a very long message that should wrap in the input bar rather than overflowing horizontally and being cut off.".to_string();
        let cursor_pos = long_input.len();

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
            composer_input: long_input,
            cursor_pos,
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
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // Render on a 45-column wide terminal
        let backend = TestBackend::new(45, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 45, 20);
                render_coordinator_pane(f, &mut app, area);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("@Architect"));
        assert!(content.contains("very long message"));
        assert!(content.contains("overflowing"));
        assert!(content.contains("cut off"));
    }

    #[test]
    fn test_sender_palette_coverage() {
        let roles = [
            ("User", Color::Green, "●"),
            ("Coordinator", Color::Magenta, "⚡"),
            ("Architect", Color::Cyan, "◆"),
            ("Reviewer", Color::Yellow, "▲"),
            ("Researcher", Color::LightBlue, "■"),
            ("TechnicalWriter", Color::LightRed, "✦"),
            ("Copywriter", Color::LightRed, "✦"),
            ("Designer", Color::LightMagenta, "★"),
            ("Marketer", Color::LightGreen, "⯈"),
        ];

        for (name, expected_color, expected_badge) in roles {
            let (color, badge) = sender_palette(name);
            assert_eq!(color, expected_color);
            assert_eq!(badge, expected_badge);
        }

        // Custom / fallback agent names should get valid deterministic colors
        let (dyn_color1, dyn_badge1) = sender_palette("DevOps");
        let (dyn_color2, dyn_badge2) = sender_palette("DevOps");
        assert_eq!(dyn_color1, dyn_color2);
        assert_eq!(dyn_badge1, dyn_badge2);
    }

    #[test]
    fn test_format_chat_message_adaptive_header_and_threading() {
        let parent = ChatMessage {
            id: "msg-parent-1234".to_string(),
            reply_to: None,
            sender: "User".to_string(),
            role: "Human Operator".to_string(),
            message: "Need a design upgrade".to_string(),
            timestamp: 1794050000,
            fallback: None,
            superseded_by: None,
        };

        let child = ChatMessage {
            id: "msg-child-5678".to_string(),
            reply_to: Some("msg-parent-1234".to_string()),
            sender: "Designer".to_string(),
            role: "Brand & UI/UX Designer".to_string(),
            message: "Here is the upgraded design for @all to review.".to_string(),
            timestamp: 1794050010,
            fallback: None,
            superseded_by: None,
        };

        let history = vec![parent.clone(), child.clone()];

        // 1. Wide terminal (width = 90): includes role badge and padding rule
        let wide_lines = format_chat_message(&child, &history, 90);
        assert!(wide_lines.len() >= 3);
        let header_str: String = wide_lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header_str.contains("Designer"));
        assert!(header_str.contains("Brand & UI/UX Designer"));
        assert!(header_str.contains("#msg-"));
        assert!(header_str.contains("───"));

        // Thread reply indicator
        let reply_str: String = wide_lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(reply_str.contains("in reply to"));
        assert!(reply_str.contains("@User"));
        assert!(reply_str.contains("#msg-"));

        // Body with mention
        let body_str: String = wide_lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(body_str.contains("@all"));

        // 2. Narrow terminal (width = 40): compacts role to avoid wrapping
        let narrow_lines = format_chat_message(&child, &history, 40);
        let narrow_header: String = narrow_lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(narrow_header.contains("Designer"));
        assert!(!narrow_header.contains("Brand & UI/UX Designer"));
    }

    #[test]
    fn test_coordinator_selection_highlight_and_buffer_capture() {
        use crate::app::PaneSelection;
        use crate::config::{parse_prefix_key, AppConfig};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
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
            composer_input: "Test Coordinator Input".to_string(),
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
                end: (5, 0),
            }),
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: 0,
            coordinator_buffer: Vec::new(),
            clipboard: None,
            active_tasks: HashMap::new(),
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 80, 24);
                render_coordinator_pane(f, &mut app, area);
            })
            .unwrap();

        // 1. Verify coordinator_buffer was populated
        assert!(!app.coordinator_buffer.is_empty());

        // 2. Verify inner bounding rectangle exists
        let (_, inner) = app.pane_bounds.get(&0).copied().unwrap();

        // 3. Verify selected cells received highlight style Color::Rgb(50, 70, 100)
        let buffer = terminal.backend().buffer();
        for col in 0..=5 {
            let x = inner.x + col;
            let y = inner.y;
            let cell = buffer.cell((x, y)).unwrap();
            assert_eq!(cell.style().bg, Some(Color::Rgb(50, 70, 100)));
            assert_eq!(cell.style().fg, Some(Color::White));
        }

        // 4. Verify extraction from coordinator_buffer
        let extracted = app.extract_selected_text(&app.mouse_selection.clone().unwrap());
        assert!(extracted.is_some());
    }

    #[test]
    fn test_format_thinking_card() {
        let parent_msg = ChatMessage {
            id: "msg-user-1234".to_string(),
            reply_to: None,
            sender: "User".to_string(),
            role: "Human Operator".to_string(),
            message: "Analyze the architecture".to_string(),
            timestamp: 1794050000,
            fallback: None,
            superseded_by: None,
        };

        let task = ActiveTask {
            agent_name: "Reviewer".to_string(),
            agent_role: "Reviewer & Auditor".to_string(),
            task_ref: "msg-user-1234".to_string(),
            start_time: std::time::Instant::now(),
        };

        let history = vec![parent_msg];
        let lines = format_thinking_card(&task, &history, 80);

        assert!(lines.len() >= 3);
        let header_str: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header_str.contains("Reviewer"));
        assert!(header_str.contains("Reviewer & Auditor"));
        assert!(header_str.contains("THINKING"));

        let reply_str: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(reply_str.contains("processing task from @User"));
        assert!(reply_str.contains("#msg-"));

        let body_str: String = lines[2].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(body_str.contains("Generating response..."));
    }

    #[test]
    fn test_render_coordinator_pane_thinking_card() {
        let mut app = AppState {
            config: AppConfig::default(),
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
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        app.active_tasks.insert(
            "reviewer".to_string(),
            ActiveTask {
                agent_name: "Reviewer".to_string(),
                agent_role: "Reviewer & Auditor".to_string(),
                task_ref: "ref-1111".to_string(),
                start_time: std::time::Instant::now(),
            },
        );

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 80, 20);
                render_coordinator_pane(f, &mut app, area);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered_text: String = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered_text.contains("Reviewer"));
        assert!(rendered_text.contains("THINKING"));
        assert!(rendered_text.contains("Generating response..."));
    }

    #[test]
    fn test_composer_tab_title_clipping_narrow_terminal() {
        use crate::app::TabCompletionState;
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
            composer_input: "@".to_string(),
            cursor_pos: 1,
            chat_history: vec![],
            known_message_ids: HashSet::new(),
            initial_message_ids: HashSet::new(),
            clients: vec![],
            active_chain_depth: 0,
            max_chain_depth: 100,
            chat_file_path: PathBuf::from("test.jsonl"),
            pty_dimensions: HashMap::new(),
            collaborative_next_idx: 0,
            tab_completion: Some(TabCompletionState {
                original_prefix: String::new(),
                at_index: 0,
                matches: vec![
                    "Architect".to_string(),
                    "Reviewer".to_string(),
                    "TechnicalWriter".to_string(),
                    "Designer".to_string(),
                    "Marketer".to_string(),
                    "Researcher".to_string(),
                    "all".to_string(),
                ],
                current_idx: 0,
                last_replaced_len: 0,
            }),
            initialized_clients: HashSet::new(),
            mouse_selection: None,
            pane_bounds: HashMap::new(),
            chat_scroll_offset: 0,
            pane_scroll_offsets: HashMap::new(),
            last_chat_offset: 0,
            coordinator_buffer: Vec::new(),
            clipboard: None,
            active_tasks: HashMap::new(),
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        let render = |app: &mut AppState| -> Vec<String> {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|f| {
                    let area = Rect::new(0, 0, 80, 24);
                    render_coordinator_pane(f, app, area);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer.cell((x, y)).unwrap().symbol())
                        .collect::<String>()
                })
                .collect()
        };

        // Active tab completion: canonical name in list, alias never injected
        let rows = render(&mut app);
        let content: String = rows.join("\n");
        assert!(
            content.contains("@TechnicalWriter"),
            "canonical name must appear in the tab list"
        );
        assert!(
            !content.contains("@TechWriter"),
            "alias must not appear as a completion candidate"
        );

        // No wrap/corruption: title row keeps a right-border corner at column 79
        let title_y = rows
            .iter()
            .position(|r| r.contains("Composer Input"))
            .expect("composer title row must render");
        let title_row = &rows[title_y];
        let last = title_row.chars().last().unwrap();
        assert!(
            title_row.ends_with("┐│") || title_row.ends_with("┤│"),
            "composer corner + pane border must remain intact; got '{last}'; row: {title_row}"
        );

        // Trailing hint clipped on 80-col terminal during active completion
        assert!(
            !content.contains("Shift+Enter"),
            "hint must be clipped while the tab list is active"
        );

        // Completion cleared: full hint visible again
        app.tab_completion = None;
        let rows_after = render(&mut app);
        assert!(rows_after.join("\n").contains("Shift+Enter"));
    }

    #[test]
    fn test_render_cache_width_invalidation() {
        use crate::chat::ChatMessage;
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
            chat_history: vec![
                ChatMessage::new_user("First long message that wraps across lines when terminal is narrow.".to_string()),
                ChatMessage::new_user("Second message with **markdown** and `code`.".to_string()),
            ],
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
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // Render at 80 cols
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 80, 24);
                render_coordinator_pane(f, &mut app, area);
            })
            .unwrap();

        let initial_width = app.chat_cache_width;
        let initial_total = app.chat_total_lines;
        assert!(initial_width > 0);
        assert!(initial_total > 0);
        assert_eq!(app.message_heights.len(), 2);

        // Render at 120 cols (terminal resize)
        let backend120 = TestBackend::new(120, 24);
        let mut terminal120 = Terminal::new(backend120).unwrap();
        terminal120
            .draw(|f| {
                let area = Rect::new(0, 0, 120, 24);
                render_coordinator_pane(f, &mut app, area);
            })
            .unwrap();

        assert_ne!(app.chat_cache_width, initial_width);
        assert_eq!(app.message_heights.len(), 2);
    }

    #[test]
    fn test_scroll_zero_parse_work_and_viewport_windowing() {
        use crate::chat::ChatMessage;
        use crate::config::{parse_prefix_key, AppConfig};
        use std::collections::{HashMap, HashSet};
        use std::path::PathBuf;

        let mut msgs = Vec::with_capacity(1000);
        for i in 0..1000 {
            msgs.push(ChatMessage::new_user(format!("Log entry {i}: status normal with `code` and @Reviewer mention.")));
        }

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
            chat_history: msgs,
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
            watchdog_tasks: HashMap::new(),
            last_chat_file_size: 0,
            render_cache: HashMap::new(),
            chat_cache_width: 0,
            chat_total_lines: 0,
            message_heights: Vec::new(),
        };

        // First frame: populates cache and message heights
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 100, 30);
                render_coordinator_pane(f, &mut app, area);
            })
            .unwrap();

        let cached_total = app.chat_total_lines;
        assert!(cached_total > 1000);
        assert_eq!(app.message_heights.len(), 1000);

        // Perform 50 consecutive scroll frames: must complete in sub-millisecond time with ZERO re-wraps
        let start = std::time::Instant::now();
        for offset in 1..=50 {
            app.chat_scroll_offset = offset * 5;
            terminal
                .draw(|f| {
                    let area = Rect::new(0, 0, 100, 30);
                    render_coordinator_pane(f, &mut app, area);
                })
                .unwrap();
            // Total lines remains identical (no re-wrap!)
            assert_eq!(app.chat_total_lines, cached_total);
        }
        let elapsed = start.elapsed();
        // 50 scroll frames in unoptimized debug test harness should complete smoothly (< 500ms total)
        assert!(elapsed.as_millis() < 500, "50 scroll frames took too long: {:?}", elapsed);
    }
}
