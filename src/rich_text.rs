#![deny(clippy::indexing_slicing)]
#![deny(clippy::string_slice)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use regex::Regex;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

static INLINE_MENTION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@([a-zA-Z0-9_\-]+)").expect("Invalid mention regex"));

/// Safe truncation of an ID (e.g. UUID) by character count without byte-slicing panic.
pub fn safe_truncate_id(id: &str, max_chars: usize) -> String {
    id.chars().take(max_chars).collect()
}

/// Calculate the display width of a string in terminal cells.
pub fn display_width(s: &str) -> usize {
    s.width()
}

/// Truncate a string to fit within `max_width` terminal columns based on grapheme clusters.
/// Multi-codepoint graphemes (e.g. ZWJ emoji, flags, skin tones) are never cut mid-sequence.
pub fn safe_truncate_graphemes(s: &str, max_width: usize, ellipsis: bool) -> String {
    let mut current_width: usize = 0;
    let mut result = String::new();
    let target_width = if ellipsis && max_width > 1 {
        max_width.saturating_sub(1)
    } else {
        max_width
    };

    for g in s.graphemes(true) {
        let gw = g.width();
        if current_width.saturating_add(gw) > target_width {
            if ellipsis {
                result.push('…');
            }
            return result;
        }
        result.push_str(g);
        current_width = current_width.saturating_add(gw);
    }
    result
}

/// Normalizes text by removing/neutralizing dangerous characters:
/// - Strips Bidi override controls (U+202A..=U+202E, U+2066..=U+2069, U+200E, U+200F).
/// - Normalizes `\r\n` and lone `\r` to `\n`.
/// - Normalizes tabs to 4 spaces.
pub fn normalize_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // Normalize CRLF to LF
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            // Expand tab to 4 spaces
            '\t' => {
                out.push_str("    ");
            }
            // Neutralize Bidi overrides & invisible directional marks
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' => {
                // Strip bidi overrides to prevent visual spoofing
            }
            other => {
                out.push(other);
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsiToken {
    Text(String),
    Sgr(StyleUpdate),
    Neutralized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleUpdate {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub add_modifier: Modifier,
    pub sub_modifier: Modifier,
    pub reset: bool,
}

impl StyleUpdate {
    pub fn apply_to(&self, style: &mut Style) {
        if self.reset {
            *style = Style::default();
        }
        if let Some(fg) = self.fg {
            *style = style.fg(fg);
        }
        if let Some(bg) = self.bg {
            *style = style.bg(bg);
        }
        if !self.add_modifier.is_empty() {
            *style = style.add_modifier(self.add_modifier);
        }
        if !self.sub_modifier.is_empty() {
            *style = style.remove_modifier(self.sub_modifier);
        }
    }
}

/// Scans a line for ANSI escape sequences.
/// Whitelist: ONLY CSI-SGR (`\x1b[...m`) is decoded into `StyleUpdate`.
/// All other escapes (OSC, non-SGR CSI, C1, C0 controls) are neutralized into visible caret notation.
pub fn scan_ansi(line: &str) -> Vec<AnsiToken> {
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    let mut text_buf = String::new();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if !text_buf.is_empty() {
                tokens.push(AnsiToken::Text(std::mem::take(&mut text_buf)));
            }

            match chars.peek() {
                Some(&'[') => {
                    chars.next(); // consume '['
                    // CSI sequence: read parameter & intermediate bytes, then final byte
                    let mut seq = String::new();
                    let mut final_byte = None;

                    while let Some(&next_c) = chars.peek() {
                        if (0x40..=0x7E).contains(&(next_c as u32)) {
                            final_byte = Some(next_c);
                            chars.next();
                            break;
                        } else if (0x20..=0x3F).contains(&(next_c as u32)) {
                            seq.push(next_c);
                            chars.next();
                        } else {
                            // Unexpected byte or terminator
                            break;
                        }
                    }

                    if final_byte == Some('m') {
                        // Whitelisted SGR sequence!
                        let update = parse_sgr(&seq);
                        tokens.push(AnsiToken::Sgr(update));
                    } else {
                        // Disallowed CSI (e.g. cursor moves, clear screen 2J, scroll regions)
                        let f = final_byte.map(|b| b.to_string()).unwrap_or_default();
                        let caret = format!("^[[{}{}", seq, f);
                        tokens.push(AnsiToken::Neutralized(caret));
                    }
                }
                Some(&']') => {
                    chars.next(); // consume ']'
                    // OSC sequence: read until BEL (\x07) or ST (\x1b\\) or EOF
                    let mut osc_content = String::new();
                    while let Some(next_c) = chars.next() {
                        if next_c == '\x07' {
                            break;
                        } else if next_c == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        } else if next_c == '\n' {
                            break;
                        } else {
                            osc_content.push(next_c);
                        }
                    }
                    let caret = format!("^[OSC:{}^]", osc_content);
                    tokens.push(AnsiToken::Neutralized(caret));
                }
                Some(&other) => {
                    // Non-CSI / non-OSC escape
                    chars.next();
                    let caret = format!("^[{other}");
                    tokens.push(AnsiToken::Neutralized(caret));
                }
                None => {
                    // Unterminated ESC at end of string
                    tokens.push(AnsiToken::Neutralized("^[".to_string()));
                }
            }
        } else if c == '\u{9B}' {
            // 8-bit C1 CSI (disallowed)
            if !text_buf.is_empty() {
                tokens.push(AnsiToken::Text(std::mem::take(&mut text_buf)));
            }
            tokens.push(AnsiToken::Neutralized("^[C1:".to_string()));
        } else if (c as u32) < 0x20 && c != '\n' {
            // C0 controls neutralized (e.g. \x00 -> ^@, \x07 -> ^G)
            if !text_buf.is_empty() {
                tokens.push(AnsiToken::Text(std::mem::take(&mut text_buf)));
            }
            let caret = format!("^{}", ((c as u8).saturating_add(b'@')) as char);
            tokens.push(AnsiToken::Neutralized(caret));
        } else {
            text_buf.push(c);
        }
    }

    if !text_buf.is_empty() {
        tokens.push(AnsiToken::Text(text_buf));
    }

    tokens
}

fn parse_sgr(params: &str) -> StyleUpdate {
    let mut update = StyleUpdate::default();
    if params.is_empty() {
        update.reset = true;
        return update;
    }

    let parts: Vec<&str> = params.split(';').collect();
    let mut idx = 0;

    while let Some(&part) = parts.get(idx) {
        let code = part.parse::<u16>().unwrap_or(0);
        match code {
            0 => {
                update.reset = true;
                idx = idx.saturating_add(1);
            }
            1 => {
                update.add_modifier |= Modifier::BOLD;
                idx = idx.saturating_add(1);
            }
            2 => {
                update.add_modifier |= Modifier::DIM;
                idx = idx.saturating_add(1);
            }
            3 => {
                update.add_modifier |= Modifier::ITALIC;
                idx = idx.saturating_add(1);
            }
            4 => {
                update.add_modifier |= Modifier::UNDERLINED;
                idx = idx.saturating_add(1);
            }
            9 => {
                update.add_modifier |= Modifier::CROSSED_OUT;
                idx = idx.saturating_add(1);
            }
            22 => {
                update.sub_modifier |= Modifier::BOLD | Modifier::DIM;
                idx = idx.saturating_add(1);
            }
            23 => {
                update.sub_modifier |= Modifier::ITALIC;
                idx = idx.saturating_add(1);
            }
            24 => {
                update.sub_modifier |= Modifier::UNDERLINED;
                idx = idx.saturating_add(1);
            }
            29 => {
                update.sub_modifier |= Modifier::CROSSED_OUT;
                idx = idx.saturating_add(1);
            }
            30..=37 => {
                update.fg = Some(ansi_basic_color(code.saturating_sub(30)));
                idx = idx.saturating_add(1);
            }
            39 => {
                update.fg = Some(Color::Reset);
                idx = idx.saturating_add(1);
            }
            40..=47 => {
                update.bg = Some(ansi_basic_color(code.saturating_sub(40)));
                idx = idx.saturating_add(1);
            }
            49 => {
                update.bg = Some(Color::Reset);
                idx = idx.saturating_add(1);
            }
            90..=97 => {
                update.fg = Some(ansi_bright_color(code.saturating_sub(90)));
                idx = idx.saturating_add(1);
            }
            100..=107 => {
                update.bg = Some(ansi_bright_color(code.saturating_sub(100)));
                idx = idx.saturating_add(1);
            }
            38 => {
                if parts.get(idx.saturating_add(1)).copied() == Some("5") {
                    if let Some(color_idx) = parts
                        .get(idx.saturating_add(2))
                        .and_then(|p| p.parse::<u8>().ok())
                    {
                        update.fg = Some(Color::Indexed(color_idx));
                    }
                    idx = idx.saturating_add(3);
                } else if parts.get(idx.saturating_add(1)).copied() == Some("2") {
                    let r = parts.get(idx.saturating_add(2)).and_then(|p| p.parse::<u8>().ok());
                    let g = parts.get(idx.saturating_add(3)).and_then(|p| p.parse::<u8>().ok());
                    let b = parts.get(idx.saturating_add(4)).and_then(|p| p.parse::<u8>().ok());
                    if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                        update.fg = Some(Color::Rgb(r, g, b));
                    }
                    idx = idx.saturating_add(5);
                } else {
                    idx = idx.saturating_add(1);
                }
            }
            48 => {
                if parts.get(idx.saturating_add(1)).copied() == Some("5") {
                    if let Some(color_idx) = parts
                        .get(idx.saturating_add(2))
                        .and_then(|p| p.parse::<u8>().ok())
                    {
                        update.bg = Some(Color::Indexed(color_idx));
                    }
                    idx = idx.saturating_add(3);
                } else if parts.get(idx.saturating_add(1)).copied() == Some("2") {
                    let r = parts.get(idx.saturating_add(2)).and_then(|p| p.parse::<u8>().ok());
                    let g = parts.get(idx.saturating_add(3)).and_then(|p| p.parse::<u8>().ok());
                    let b = parts.get(idx.saturating_add(4)).and_then(|p| p.parse::<u8>().ok());
                    if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                        update.bg = Some(Color::Rgb(r, g, b));
                    }
                    idx = idx.saturating_add(5);
                } else {
                    idx = idx.saturating_add(1);
                }
            }
            _ => {
                idx = idx.saturating_add(1);
            }
        }
    }

    update
}

fn ansi_basic_color(code: u16) -> Color {
    match code {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn ansi_bright_color(code: u16) -> Color {
    match code {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

/// Renders a full rich text message into Ratatui `Line`s.
/// Supports fenced code blocks, headers, blockquotes, lists, dividers,
/// inline bold/italic/code, SGR ANSI colors, and integrated @mentions.
pub fn render_rich_message_body<F>(
    raw_message: &str,
    available_width: usize,
    palette_lookup: F,
) -> Vec<Line<'static>>
where
    F: Fn(&str) -> (Color, &'static str),
{
    let normalized = normalize_text(raw_message);
    let mut lines = Vec::new();
    let mut in_code_fence = false;
    let mut fence_lang = String::new();

    for line in normalized.lines() {
        let trimmed = line.trim();

        // 1. Code Fence Toggle
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if in_code_fence {
                // Closing fence
                in_code_fence = false;
                fence_lang.clear();
                continue;
            } else {
                // Opening fence
                in_code_fence = true;
                let rest = if let Some(stripped) = trimmed.strip_prefix("```") {
                    stripped.trim()
                } else if let Some(stripped) = trimmed.strip_prefix("~~~") {
                    stripped.trim()
                } else {
                    ""
                };
                fence_lang = rest.to_string();

                let header_title = if !fence_lang.is_empty() {
                    format!("  ┌─ [Code: {}] ──", fence_lang)
                } else {
                    "  ┌─ [Code] ──".to_string()
                };
                let pad = available_width.saturating_sub(display_width(&header_title).saturating_add(1));
                let mut header_spans = vec![Span::styled(
                    header_title,
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                )];
                if pad > 0 {
                    header_spans.push(Span::styled("─".repeat(pad), Style::default().fg(Color::DarkGray)));
                }
                lines.push(Line::from(header_spans));
                continue;
            }
        }

        // Inside code block: code is literal, mentions suppressed!
        if in_code_fence {
            let spans = vec![
                Span::styled("  │ ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Rgb(220, 220, 225)).bg(Color::Rgb(30, 30, 36)),
                ),
            ];
            lines.push(Line::from(spans));
            continue;
        }

        if trimmed.is_empty() {
            lines.push(Line::from(""));
            continue;
        }

        // 2. Horizontal Divider (standalone ---, ***, ___)
        if (trimmed.starts_with("---") || trimmed.starts_with("***") || trimmed.starts_with("___"))
            && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_' || c.is_whitespace())
        {
            let divider_len = available_width.saturating_sub(4).max(4);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("─".repeat(divider_len), Style::default().fg(Color::DarkGray)),
            ]));
            continue;
        }

        // 3. Blockquote (> quote)
        if let Some(quote_content) = trimmed.strip_prefix('>') {
            let quote_trimmed = quote_content.trim_start();
            let mut spans = vec![
                Span::styled("  │ ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            ];
            let inline_spans = render_inline_rich_text(
                quote_trimmed,
                Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
                &palette_lookup,
                true, // allow mentions
            );
            spans.extend(inline_spans);
            lines.push(Line::from(spans));
            continue;
        }

        // 4. Headers (# H1, ## H2, ### H3, #### H4)
        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&hashes) && trimmed.get(hashes..).is_some_and(|s| s.starts_with(' ')) {
                let header_text = trimmed.get(hashes..).unwrap_or("").trim_start();
                let (prefix, style) = match hashes {
                    1 => ("# ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    2 => ("## ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    3 => ("### ", Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD)),
                    _ => ("#### ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                };
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(prefix, style),
                ];
                let inline_spans = render_inline_rich_text(header_text, style, &palette_lookup, true);
                spans.extend(inline_spans);
                lines.push(Line::from(spans));
                continue;
            }
        }

        // 5. Bullet and Numbered Lists
        let (is_list, bullet_prefix, list_content) = if let Some(stripped) = trimmed.strip_prefix("- ") {
            (true, "• ".to_string(), stripped)
        } else if let Some(stripped) = trimmed.strip_prefix("* ") {
            (true, "• ".to_string(), stripped)
        } else if let Some(dot_idx) = trimmed.find(". ") {
            let num_str = trimmed.get(..dot_idx).unwrap_or("");
            if dot_idx > 0 && num_str.chars().all(|c| c.is_ascii_digit()) {
                let prefix = trimmed.get(..dot_idx.saturating_add(2)).unwrap_or("").to_string();
                let content = trimmed.get(dot_idx.saturating_add(2)..).unwrap_or("");
                (true, prefix, content)
            } else {
                (false, String::new(), "")
            }
        } else {
            (false, String::new(), "")
        };

        if is_list {
            let mut spans = vec![
                Span::raw("  "),
                Span::styled(bullet_prefix, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            ];
            let inline_spans = render_inline_rich_text(
                list_content,
                Style::default(),
                &palette_lookup,
                true,
            );
            spans.extend(inline_spans);
            lines.push(Line::from(spans));
            continue;
        }

        // 6. Regular prose paragraph line
        let mut spans = vec![Span::raw("  ")];
        let inline_spans = render_inline_rich_text(line, Style::default(), &palette_lookup, true);
        spans.extend(inline_spans);
        lines.push(Line::from(spans));
    }

    // Unclosed code fence extends to end of message per R2.2
    if in_code_fence {
        lines.push(Line::from(vec![
            Span::styled("  └─ [End of Code] ──", Style::default().fg(Color::DarkGray)),
        ]));
    }

    lines
}

/// Parses inline formatting: inline code (`` `code` ``), bold (`**bold**`), italic (`*italic*`),
/// strikethrough (`~~text~~`), SGR ANSI sequences, and @mentions (outside code).
fn render_inline_rich_text<F>(
    raw_text: &str,
    base_style: Style,
    palette_lookup: &F,
    allow_mentions: bool,
) -> Vec<Span<'static>>
where
    F: Fn(&str) -> (Color, &'static str),
{
    let ansi_tokens = scan_ansi(raw_text);
    let mut spans = Vec::new();
    let mut current_style = base_style;

    for token in ansi_tokens {
        match token {
            AnsiToken::Sgr(update) => {
                update.apply_to(&mut current_style);
            }
            AnsiToken::Neutralized(caret_str) => {
                spans.push(Span::styled(
                    caret_str,
                    Style::default().fg(Color::LightRed).add_modifier(Modifier::DIM),
                ));
            }
            AnsiToken::Text(text) => {
                let markdown_spans = parse_markdown_spans(&text, current_style, palette_lookup, allow_mentions);
                spans.extend(markdown_spans);
            }
        }
    }

    spans
}

/// Flat, linear-time, non-recursive inline markdown parser.
/// R2.1: Never recurses on unclosed delimiters, eliminating stack overflow and quadratic backtracking.
fn parse_markdown_spans<F>(
    text: &str,
    active_style: Style,
    palette_lookup: &F,
    allow_mentions: bool,
) -> Vec<Span<'static>>
where
    F: Fn(&str) -> (Color, &'static str),
{
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut idx = 0;
    let len = chars.len();

    let mut buf = String::new();

    let flush_buf = |buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style| {
        if !buf.is_empty() {
            if allow_mentions {
                let mention_spans = highlight_mentions(buf, style, palette_lookup);
                spans.extend(mention_spans);
            } else {
                spans.push(Span::styled(buf.clone(), style));
            }
            buf.clear();
        }
    };

    while idx < len {
        let curr = chars.get(idx).copied().unwrap_or('\0');

        // 1. Inline code span: `code`
        if curr == '`' {
            flush_buf(&mut buf, &mut spans, active_style);
            idx = idx.saturating_add(1);
            let mut code_buf = String::new();
            while idx < len {
                let c = chars.get(idx).copied().unwrap_or('\0');
                if c == '`' {
                    idx = idx.saturating_add(1);
                    break;
                }
                code_buf.push(c);
                idx = idx.saturating_add(1);
            }
            // Code span suppresses all other formatting and mentions!
            let code_style = Style::default()
                .fg(Color::Rgb(230, 200, 110))
                .bg(Color::Rgb(40, 42, 50));
            spans.push(Span::styled(code_buf, code_style));
            continue;
        }

        // 2. Bold (**bold** or __bold__)
        let next_c = chars.get(idx.saturating_add(1)).copied();
        if (curr == '*' || curr == '_') && next_c == Some(curr) {
            let marker = curr;
            flush_buf(&mut buf, &mut spans, active_style);
            idx = idx.saturating_add(2);
            let mut bold_buf = String::new();
            let mut closed = false;
            while idx < len {
                let c1 = chars.get(idx).copied().unwrap_or('\0');
                let c2 = chars.get(idx.saturating_add(1)).copied().unwrap_or('\0');
                if c1 == marker && c2 == marker {
                    closed = true;
                    idx = idx.saturating_add(2);
                    break;
                }
                bold_buf.push(c1);
                idx = idx.saturating_add(1);
            }
            let bold_style = active_style.add_modifier(Modifier::BOLD);
            if allow_mentions {
                spans.extend(highlight_mentions(&bold_buf, bold_style, palette_lookup));
            } else {
                spans.push(Span::styled(bold_buf, bold_style));
            }
            let _ = closed;
            continue;
        }

        // 3. Strikethrough (~~text~~)
        if curr == '~' && next_c == Some('~') {
            flush_buf(&mut buf, &mut spans, active_style);
            idx = idx.saturating_add(2);
            let mut strike_buf = String::new();
            while idx < len {
                let c1 = chars.get(idx).copied().unwrap_or('\0');
                let c2 = chars.get(idx.saturating_add(1)).copied().unwrap_or('\0');
                if c1 == '~' && c2 == '~' {
                    idx = idx.saturating_add(2);
                    break;
                }
                strike_buf.push(c1);
                idx = idx.saturating_add(1);
            }
            let strike_style = active_style.add_modifier(Modifier::CROSSED_OUT);
            if allow_mentions {
                spans.extend(highlight_mentions(&strike_buf, strike_style, palette_lookup));
            } else {
                spans.push(Span::styled(strike_buf, strike_style));
            }
            continue;
        }

        // 4. Italic (*italic* or _italic_)
        if curr == '*' || curr == '_' {
            let marker = curr;
            let prev_c = if idx > 0 {
                chars.get(idx.saturating_sub(1)).copied()
            } else {
                None
            };
            let is_escaped_or_word = marker == '_' && prev_c.is_some_and(|c| c.is_alphanumeric());

            if is_escaped_or_word {
                buf.push(curr);
                idx = idx.saturating_add(1);
                continue;
            }

            flush_buf(&mut buf, &mut spans, active_style);
            idx = idx.saturating_add(1);
            let mut italic_buf = String::new();
            while idx < len {
                let c = chars.get(idx).copied().unwrap_or('\0');
                if c == marker {
                    idx = idx.saturating_add(1);
                    break;
                }
                italic_buf.push(c);
                idx = idx.saturating_add(1);
            }
            let italic_style = active_style.add_modifier(Modifier::ITALIC);
            if allow_mentions {
                spans.extend(highlight_mentions(&italic_buf, italic_style, palette_lookup));
            } else {
                spans.push(Span::styled(italic_buf, italic_style));
            }
            continue;
        }

        buf.push(curr);
        idx = idx.saturating_add(1);
    }

    flush_buf(&mut buf, &mut spans, active_style);
    spans
}

fn highlight_mentions<F>(text: &str, base_style: Style, palette_lookup: &F) -> Vec<Span<'static>>
where
    F: Fn(&str) -> (Color, &'static str),
{
    let mut spans = Vec::new();
    let mut last_idx = 0;

    for mat in INLINE_MENTION_REGEX.find_iter(text) {
        if mat.start() > last_idx
            && let Some(prefix) = text.get(last_idx..mat.start()) {
                spans.push(Span::styled(prefix.to_string(), base_style));
            }
        let mention_str = mat.as_str();
        let target_name = mention_str.get(1..).unwrap_or("");
        let style = if target_name.eq_ignore_ascii_case("all") {
            base_style.fg(Color::Magenta).add_modifier(Modifier::BOLD)
        } else {
            let (col, _) = palette_lookup(target_name);
            base_style.fg(col).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(mention_str.to_string(), style));
        last_idx = mat.end();
    }

    if last_idx < text.len()
        && let Some(suffix) = text.get(last_idx..) {
            spans.push(Span::styled(suffix.to_string(), base_style));
        }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_palette(name: &str) -> (Color, &'static str) {
        match name {
            "Architect" => (Color::Cyan, "Lead Architect"),
            "Reviewer" => (Color::Green, "Reviewer"),
            _ => (Color::Yellow, "Peer"),
        }
    }

    #[test]
    fn test_sgr_ansi_decoding() {
        let input = "\x1b[31mRed\x1b[0m \x1b[1;34mBold Blue\x1b[0m \x1b[38;5;196m256-Red\x1b[0m";
        let spans = render_inline_rich_text(input, Style::default(), &dummy_palette, true);
        assert!(!spans.is_empty());
        assert_eq!(spans.first().map(|s| s.content.as_ref()), Some("Red"));
        assert_eq!(spans.first().and_then(|s| s.style.fg), Some(Color::Red));
        assert_eq!(spans.get(2).map(|s| s.content.as_ref()), Some("Bold Blue"));
        assert_eq!(spans.get(2).and_then(|s| s.style.fg), Some(Color::Blue));
        assert!(spans.get(2).is_some_and(|s| s.style.add_modifier.contains(Modifier::BOLD)));
        assert_eq!(spans.get(4).map(|s| s.content.as_ref()), Some("256-Red"));
        assert_eq!(spans.get(4).and_then(|s| s.style.fg), Some(Color::Indexed(196)));
    }

    #[test]
    fn test_neutralize_disallowed_ansi() {
        // Disallowed CSI: clear screen, cursor home
        let input = "Danger: \x1b[2J\x1b[H Clear Screen";
        let tokens = scan_ansi(input);
        assert_eq!(tokens.first(), Some(&AnsiToken::Text("Danger: ".to_string())));
        assert_eq!(tokens.get(1), Some(&AnsiToken::Neutralized("^[[2J".to_string())));
        assert_eq!(tokens.get(2), Some(&AnsiToken::Neutralized("^[[H".to_string())));

        // Disallowed OSC: Title spoof, clipboard exfiltration
        let osc_title = "Title \x1b]0;Spoofed Title\x07 Done";
        let tokens_title = scan_ansi(osc_title);
        assert_eq!(tokens_title.get(1), Some(&AnsiToken::Neutralized("^[OSC:0;Spoofed Title^]".to_string())));

        let osc_clip = "\x1b]52;c;ZXhmaWw=\x07";
        let tokens_clip = scan_ansi(osc_clip);
        assert_eq!(tokens_clip.first(), Some(&AnsiToken::Neutralized("^[OSC:52;c;ZXhmaWw=^]".to_string())));

        // C0 controls
        let c0_input = "Hello\x00\x07World";
        let tokens_c0 = scan_ansi(c0_input);
        assert_eq!(tokens_c0.get(1), Some(&AnsiToken::Neutralized("^@".to_string())));
        assert_eq!(tokens_c0.get(2), Some(&AnsiToken::Neutralized("^G".to_string())));

        // C1 CSI
        let c1_input = "C1: \u{9B}31m";
        let tokens_c1 = scan_ansi(c1_input);
        assert_eq!(tokens_c1.get(1), Some(&AnsiToken::Neutralized("^[C1:".to_string())));

        // Bare ESC at EOF
        let bare_esc = "Bare \x1b";
        let tokens_bare = scan_ansi(bare_esc);
        assert_eq!(tokens_bare.get(1), Some(&AnsiToken::Neutralized("^[".to_string())));
    }

    #[test]
    fn test_code_span_suppresses_mentions() {
        let input = "Check this code: `@Reviewer fix bug` and @Reviewer in prose.";
        let lines = render_rich_message_body(input, 80, dummy_palette);
        assert_eq!(lines.len(), 1);
        let line = lines.first().unwrap();
        let code_span = line.spans.get(2).unwrap();
        assert_eq!(code_span.content, "@Reviewer fix bug");
        assert_ne!(code_span.style.fg, Some(Color::Green)); // Not reviewer green!

        // Further down in prose, @Reviewer IS highlighted with Reviewer's green!
        let prose_mention = line.spans.iter().find(|s| s.content == "@Reviewer").unwrap();
        assert_eq!(prose_mention.style.fg, Some(Color::Green));
    }

    #[test]
    fn test_code_fence_block_suppresses_mentions() {
        let input = "```rust\nfn test() {\n    // @Reviewer should be literal\n}\n```\n@Reviewer outside";
        let lines = render_rich_message_body(input, 80, dummy_palette);
        assert!(lines.len() >= 4);

        // Line inside code block:
        let code_line = lines.get(2).unwrap();
        let mention_span = code_line.spans.iter().find(|s| s.content.contains("@Reviewer")).unwrap();
        assert_ne!(mention_span.style.fg, Some(Color::Green));

        // Line outside code block:
        let outside_line = lines.last().unwrap();
        let mention_outside = outside_line.spans.iter().find(|s| s.content == "@Reviewer").unwrap();
        assert_eq!(mention_outside.style.fg, Some(Color::Green));
    }

    #[test]
    fn test_markdown_block_elements() {
        let input = "# Heading 1\n## Heading 2\n> Blockquote with @Architect\n- List item\n1. Numbered item\n---";
        let lines = render_rich_message_body(input, 80, dummy_palette);
        assert_eq!(lines.len(), 6);

        // Header 1
        assert!(lines.first().and_then(|l| l.spans.get(1)).is_some_and(|s| s.content.contains('#')));
        assert_eq!(lines.first().and_then(|l| l.spans.get(1)).and_then(|s| s.style.fg), Some(Color::Cyan));

        // Blockquote
        assert!(lines.get(2).and_then(|l| l.spans.first()).is_some_and(|s| s.content.contains('│')));
        let bq_mention = lines.get(2).unwrap().spans.iter().find(|s| s.content == "@Architect").unwrap();
        assert_eq!(bq_mention.style.fg, Some(Color::Cyan));

        // Divider
        assert!(lines.get(5).and_then(|l| l.spans.get(1)).is_some_and(|s| s.content.contains('─')));
    }

    #[test]
    fn test_bidi_override_neutralization() {
        let hostile = "Hello \u{202A}spoofed\u{202C} text";
        let normalized = normalize_text(hostile);
        assert!(!normalized.contains('\u{202A}'));
        assert!(!normalized.contains('\u{202C}'));
        assert_eq!(normalized, "Hello spoofed text");
    }

    #[test]
    fn test_emoji_and_grapheme_width_safety() {
        let emojis = "Family: 👨‍👩‍👧‍👦, Flag: 🇺🇸, Thumbs: 👍🏽, Accent: e\u{301}";
        let truncated = safe_truncate_graphemes(emojis, 15, true);
        assert!(truncated.ends_with('…'));
        assert!(truncated.is_char_boundary(truncated.len()));

        assert_eq!(display_width("🚀"), 2);
        assert_eq!(display_width("Hello"), 5);
        assert_eq!(safe_truncate_id("547f7be9-3053-4194-81b4-118fbce39ba9", 4), "547f");
    }

    #[test]
    fn test_adversarial_100kb_unclosed_delimiter_recursion_prevention() {
        // R2.1 test: 100KB adversarial line with interleaved unclosed delimiters
        let pattern = "***a***a~~b~~*c*";
        let adversarial = pattern.repeat(6250); // ~100KB string
        assert!(adversarial.len() >= 100_000);

        let start = std::time::Instant::now();
        let spans = render_inline_rich_text(&adversarial, Style::default(), &dummy_palette, true);
        let elapsed = start.elapsed();

        // Must complete in linear time (< 150ms) without stack overflow
        assert!(elapsed.as_millis() < 500, "Adversarial line took too long: {:?}", elapsed);
        assert!(!spans.is_empty(), "Must produce spans without panicking");
    }

    #[test]
    fn test_property_fuzz_no_panic() {
        let long_str = "a".repeat(1000);
        let test_vectors = [
            "\x1b[2J",
            "\x1b[H",
            "\x1b]0;pwn\x07",
            "\x1b]52;c;ZXhmaWw=\x07",
            "\x1b[38;5;196m",
            "\x1b[1;34m",
            "\x1b",
            "\u{9B}31m",
            "\x07",
            "\x00",
            "\x1b[m",
            "\x1b[;;;m",
            "******",
            "``````",
            "~~~~~~~~",
            "----",
            "> > > nested",
            "👨‍👩‍👧‍👦🇺🇸👍🏽",
            "",
            long_str.as_str(),
        ];

        for vec in test_vectors {
            let _ = render_rich_message_body(vec, 40, dummy_palette);
            let _ = safe_truncate_graphemes(vec, 10, true);
        }
    }
}
