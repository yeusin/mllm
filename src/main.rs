use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use mllm::app::AppState;
use mllm::config::{load_config, run_init};
use mllm::ui;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, stdout};
use std::panic;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(
    name = "mllm",
    version,
    about = "Tmux-Style Multi-LLM Client Coordination Tool"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to YAML configuration file [default: $HOME/.config/mllm/mllm.yaml]
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a new mllm.yaml configuration file
    Init {
        /// Target path for configuration file [default: $HOME/.config/mllm/mllm.yaml]
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Overwrite configuration file if it already exists
        #[arg(short, long)]
        force: bool,
    },
    /// Update existing mllm executable to the latest version
    Update {
        /// Check for available updates without applying them
        #[arg(short, long)]
        check: bool,

        /// Specific version or tag to install (e.g. 'v0.1.0') [default: latest]
        #[arg(short, long, default_value = "latest")]
        version: String,

        /// Force update even if already on the latest version
        #[arg(short, long)]
        force: bool,

        /// Simulate update steps without writing changes
        #[arg(long)]
        dry_run: bool,

        /// Target executable path to update [default: current executable]
        #[arg(short, long)]
        target: Option<PathBuf>,
    },
}

/// RAII guard to guarantee terminal restoration on normal exit, error return, or unwind
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            crossterm::event::DisableBracketedPaste,
            DisableMouseCapture,
            crossterm::cursor::Show
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Commands::Init { output, force }) = cli.command {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let target_path = match output {
            Some(p) => p,
            None => mllm::config::default_config_path()?,
        };
        return run_init(&mut stdin.lock(), &mut stdout, &target_path, force);
    }

    if let Some(Commands::Update {
        check,
        version,
        force,
        dry_run,
        target,
    }) = cli.command
    {
        let opts = mllm::update::UpdateOptions {
            check_only: check,
            version,
            force,
            dry_run,
            target_path: target,
        };
        let mut stdout = io::stdout();
        return mllm::update::run_update(&opts, &mut stdout);
    }

    // Determine config path and its parent directory
    let (config_path, is_default_path) = match cli.config {
        Some(p) => (p, false),
        None => (mllm::config::default_config_path()?, true),
    };
    let config_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // On first launch with default path, auto-scaffold config if it does not exist
    if is_default_path && !config_path.exists() {
        mllm::config::ensure_default_config_exists(&config_path)
            .with_context(|| format!("Failed to scaffold default configuration at '{}'", config_path.display()))?;
    } else if !config_path.exists() {
        eprintln!(
            "Notice: Configuration file '{}' not found. Run 'mllm init' to create one.",
            config_path.display()
        );
    }

    let config = load_config(&config_path)?;

    // Set up terminal crash/panic hook to guarantee terminal state restoration
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            stdout(),
            LeaveAlternateScreen,
            crossterm::event::DisableBracketedPaste,
            DisableMouseCapture
        );
        original_hook(panic_info);
    }));

    // Initialize terminal in raw mode with mouse capture for in-pane selection
    enable_raw_mode()?;
    let _terminal_guard = TerminalGuard;
    let mut stdout = stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
        EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Channel for PTY output wakeup events
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();

    // Channel for terminal input events (keyboard, resize, mouse) with high priority
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("terminal-input-reader".to_string())
        .spawn(move || loop {
            match event::read() {
                Ok(evt) => {
                    if input_tx.send(evt).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        })?;

    // Initialize application state
    let mut app = match AppState::new(config, &config_dir, event_tx) {
        Ok(a) => a,
        Err(e) => {
            let _ = disable_raw_mode();
            let _ = execute!(
                io::stdout(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                crossterm::cursor::Show
            );
            eprintln!("Error: {:#}", e);
            std::process::exit(1);
        }
    };

    let mut poll_interval = tokio::time::interval(Duration::from_millis(50));

    loop {
        // Ingest newly arrived chat log updates immediately before rendering
        app.check_chat_log_updates();

        // Redraw UI
        terminal.draw(|f| ui::render(f, &mut app))?;

        tokio::select! {
            biased;

            Some(evt) = input_rx.recv() => {
                match evt {
                    Event::Key(key) => {
                        if app.handle_key_event(key) {
                            app.shutdown();
                            cleanup_terminal(&mut terminal)?;
                            return Ok(());
                        }
                    }
                    Event::Mouse(mouse_event) => {
                        app.handle_mouse_event(mouse_event);
                    }
                    Event::Paste(text) => {
                        app.handle_paste(&text);
                    }
                    Event::Resize(_, _) => {
                        // UI layout and PTY resize happens automatically in ui::render
                    }
                    _ => {}
                }
                while let Ok(evt) = input_rx.try_recv() {
                    match evt {
                        Event::Key(key) => {
                            if app.handle_key_event(key) {
                                app.shutdown();
                                cleanup_terminal(&mut terminal)?;
                                return Ok(());
                            }
                        }
                        Event::Mouse(mouse_event) => {
                            app.handle_mouse_event(mouse_event);
                        }
                        Event::Paste(text) => {
                            app.handle_paste(&text);
                        }
                        _ => {}
                    }
                }
            }
            Some(_) = event_rx.recv() => {
                // Drain any additional queued PTY events before drawing
                while event_rx.try_recv().is_ok() {}
            }
            _ = poll_interval.tick() => {}
        }
    }
}

fn cleanup_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::event::DisableBracketedPaste,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
