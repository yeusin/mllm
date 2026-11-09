use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use vt100::Parser;

pub struct ClientPane {
    pub number: usize,
    pub name: String,
    pub role: String,
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub child: Box<dyn Child + Send + Sync>,
    pub vt_parser: Arc<Mutex<Parser>>,
    pub is_busy: Arc<AtomicBool>,
    pub last_activity_ms: Arc<std::sync::atomic::AtomicU64>,
    pub event_tx: UnboundedSender<()>,
    typing_tx: UnboundedSender<(String, std::time::Instant)>,
}

impl ClientPane {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        number: usize,
        name: String,
        role: String,
        command: &str,
        extra_args: &[String],
        working_dir: &Path,
        initial_cols: u16,
        initial_rows: u16,
        event_tx: UnboundedSender<()>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let cols = initial_cols.max(10);
        let rows = initial_rows.max(5);

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to allocate PTY")?;

        let nz_rows = NonZeroU16::new(rows).unwrap_or(NonZeroU16::new(24).unwrap());
        let nz_cols = NonZeroU16::new(cols).unwrap_or(NonZeroU16::new(80).unwrap());
        let vt_parser = Arc::new(Mutex::new(Parser::new(nz_rows, nz_cols, 1000)));

        // Parse command line using shell splitting (e.g. 'agy --dangerously-skip-permissions')
        let parts = shell_words::split(command).with_context(|| {
            format!(
                "Failed to parse command arguments for client '{}': {}",
                name, command
            )
        })?;
        if parts.is_empty() {
            anyhow::bail!(
                "Command cannot be empty for client '{}' (Pane {})",
                name,
                number
            );
        }

        let program = &parts[0];
        let mut cmd = CommandBuilder::new(program);
        cmd.cwd(working_dir);
        for arg in &parts[1..] {
            cmd.arg(arg);
        }
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLUMNS", cols.to_string());
        cmd.env("LINES", rows.to_string());

        let child = pair.slave.spawn_command(cmd).with_context(|| {
            format!(
                "Failed to spawn command '{}' for client '{}' (Pane {})",
                command, name, number
            )
        })?;

        let mut reader = pair.master.try_clone_reader()?;
        let raw_writer = pair.master.take_writer()?;
        let writer = Arc::new(Mutex::new(raw_writer));

        let last_activity_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // Background reader thread
        let parser_clone = Arc::clone(&vt_parser);
        let event_tx_clone = event_tx.clone();
        let last_activity_clone = Arc::clone(&last_activity_ms);
        std::thread::Builder::new()
            .name(format!("pty-reader-{}", number))
            .spawn(move || {
                let mut buf = [0u8; 4096];
                let mut last_hash: u64 = 0;
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut parser) = parser_clone.lock() {
                                parser.process(&buf[..n]);
                                use std::hash::{DefaultHasher, Hash, Hasher};
                                let mut hasher = DefaultHasher::new();
                                parser.screen().contents().hash(&mut hasher);
                                let h = hasher.finish();
                                if h != last_hash {
                                    last_hash = h;
                                    let now_ms = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as u64)
                                        .unwrap_or(0);
                                    last_activity_clone.store(now_ms, Ordering::SeqCst);
                                }
                            }
                            let _ = event_tx_clone.send(());
                        }
                        Err(_) => break,
                    }
                }
            })?;

        let is_busy = Arc::new(AtomicBool::new(false));
        let (typing_tx, mut typing_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, std::time::Instant)>();
        let writer_for_typing = Arc::clone(&writer);
        let busy_for_typing = Arc::clone(&is_busy);
        let event_tx_for_typing = event_tx.clone();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                while let Some((text, target_time)) = typing_rx.recv().await {
                    let now = std::time::Instant::now();
                    if target_time > now {
                        tokio::time::sleep(target_time - now).await;
                    } else {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }

                    busy_for_typing.store(true, Ordering::SeqCst);
                    let _ = event_tx_for_typing.send(());

                    // Instant copy & paste simulation via terminal bracketed paste.
                    // Pastes the entire prompt into the PTY atomically, preventing embedded
                    // newlines from early-submitting prompts, avoiding symbol autocompletion modals
                    // (such as '@' in agy), and eliminating typing lag.
                    if let Ok(mut w) = writer_for_typing.lock() {
                        let _ = w.write_all(b"\x1b[200~");
                        let _ = w.write_all(text.as_bytes());
                        let _ = w.write_all(b"\x1b[201~\r");
                        let _ = w.flush();
                    }

                    tokio::time::sleep(Duration::from_millis(50)).await;

                    busy_for_typing.store(false, Ordering::SeqCst);
                    let _ = event_tx_for_typing.send(());
                }
            });
        }

        Ok(Self {
            number,
            name,
            role,
            master: pair.master,
            writer,
            child,
            vt_parser,
            is_busy,
            last_activity_ms,
            event_tx,
            typing_tx,
        })
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(10);
        let rows = rows.max(5);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let (Some(r), Some(c)) = (NonZeroU16::new(rows), NonZeroU16::new(cols))
            && let Ok(mut parser) = self.vt_parser.lock() {
                parser.set_size(r, c);
            }
    }

    pub fn write_raw(&self, data: &[u8]) -> Result<()> {
        if let Ok(mut w) = self.writer.lock() {
            w.write_all(data)?;
            w.flush()?;
        }
        Ok(())
    }

    pub fn simulate_keyboard_input(&self, prompt: &str) {
        self.simulate_keyboard_input_with_delay(prompt, 0);
    }

    pub fn simulate_keyboard_input_with_delay(&self, prompt: &str, initial_delay_ms: u64) {
        let target_time = std::time::Instant::now() + Duration::from_millis(initial_delay_ms);
        let _ = self.typing_tx.send((prompt.to_string(), target_time));
    }

    pub fn send_nudge(&self, prompt: &str) {
        // Nudges MUST go through typing_tx (R5)
        let _ = self.typing_tx.send((prompt.to_string(), std::time::Instant::now()));
    }

    pub fn current_screen_hash(&self) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        if let Ok(parser) = self.vt_parser.lock() {
            let mut hasher = DefaultHasher::new();
            parser.screen().contents().hash(&mut hasher);
            hasher.finish()
        } else {
            0
        }
    }

    pub fn get_screen_text(&self) -> String {
        if let Ok(parser) = self.vt_parser.lock() {
            parser.screen().contents()
        } else {
            String::new()
        }
    }

    pub fn is_cursor_visible(&self) -> bool {
        if let Ok(parser) = self.vt_parser.lock() {
            !parser.screen().hide_cursor()
        } else {
            false
        }
    }

    pub fn check_status(&mut self) -> Option<u32> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.exit_code()),
            _ => None,
        }
    }

    pub fn terminate(&mut self) {
        let _ = self.child.kill();
    }
}
