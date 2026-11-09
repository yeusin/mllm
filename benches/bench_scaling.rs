use std::io::Write;
use std::time::Instant;
use tokio::sync::mpsc;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use mllm::app::AppState;
use mllm::chat::ChatMessage;
use mllm::config::{AppConfig, ClientConfig};
use mllm::ui;

fn create_app_with_clients(client_names: Vec<(&str, &str)>) -> (AppState, std::path::PathBuf) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("mllm_bench_{}_{}", std::process::id(), id));
    let _ = std::fs::create_dir_all(&tmp);
    let clients: Vec<ClientConfig> = client_names
        .into_iter()
        .enumerate()
        .map(|(i, (name, role))| ClientConfig {
            number: i + 1,
            name: name.to_string(),
            role: role.to_string(),
            command: "echo test".to_string(),
            instructions: String::new(),
            extra_args: Vec::new(),
        })
        .collect();

    let config = AppConfig {
        mandatory_protocol: String::new(),
        prefix_key: "alt".to_string(),
        general_instructions: String::new(),
        clients,
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let app = AppState::new(config, Some(&tmp), tx).expect("AppState::new");
    (app, tmp)
}

#[allow(dead_code)]
#[allow(dead_code)]
struct LatencyStats {
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    mean: f64,
    std_dev: f64,
}

fn compute_stats(mut samples: Vec<f64>) -> LatencyStats {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = samples.len();
    let p50 = samples[(len as f64 * 0.50) as usize];
    let p95 = samples[(len as f64 * 0.95) as usize];
    let p99 = samples[(len as f64 * 0.99) as usize];
    let max = samples[len - 1];
    let mean = samples.iter().sum::<f64>() / len as f64;
    let variance = samples.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / len as f64;
    let std_dev = variance.sqrt();
    LatencyStats {
        p50,
        p95,
        p99,
        max,
        mean,
        std_dev,
    }
}

fn generate_ansi_chunk(step: usize) -> Vec<u8> {
    let colors = [
        "\x1b[32m", "\x1b[34m", "\x1b[35m", "\x1b[36m", "\x1b[33m", "\x1b[1;37m",
    ];
    let color = colors[step % colors.len()];
    format!(
        "{}[Token {:04}] chunk processing step {}\x1b[0m\n",
        color, step, step
    )
    .into_bytes()
}

fn main() {
    println!("=== MLLM Scalability & Latency Benchmark Suite ===");

    // Part 1: Tab-Completion Latency across Roster Scaling
    println!("\n[Benchmark 1: Tab-Completion Resolution Across Roster Scaling]");
    let roster_counts = [3, 6, 12, 24, 48];
    for count in roster_counts {
        let clients: Vec<(String, String)> = (0..count)
            .map(|i| {
                if i == 0 {
                    ("TechnicalWriter".to_string(), "Technical Writer".to_string())
                } else {
                    (format!("AgentClient{}", i), format!("Role {}", i))
                }
            })
            .collect();
        let client_refs: Vec<(&str, &str)> = clients.iter().map(|(n, r)| (n.as_str(), r.as_str())).collect();
        let (mut app, tmp) = create_app_with_clients(client_refs);

        let mut times = Vec::with_capacity(1_000);
        for _ in 0..1_000 {
            app.composer_input = "@Tech".to_string();
            app.cursor_pos = 5;
            app.tab_completion = None;
            let start = Instant::now();
            let ok = app.try_tab_complete(true);
            assert!(ok);
            times.push(start.elapsed().as_nanos() as f64 / 1000.0);
        }
        let stats = compute_stats(times);
        println!(
            "  Roster {:2} Clients: Mean: {:5.2} µs | p50: {:5.2} µs | p95: {:5.2} µs | p99: {:5.2} µs",
            count, stats.mean, stats.p50, stats.p95, stats.p99
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Part 2: Terminal Frame Render Latency & Headroom (60 FPS = 16.66 ms)
    println!("\n[Benchmark 2: Terminal Draw Frame Stability Under Parallel Streaming]");
    let (mut app, tmp) = create_app_with_clients(vec![
        ("Architect", "Lead Architect"),
        ("Reviewer", "Reviewer & Auditor"),
        ("Researcher", "Researcher"),
        ("TechnicalWriter", "Technical Writer"),
        ("Designer", "Brand & UI/UX Designer"),
        ("Marketer", "Growth & Campaign Specialist"),
    ]);

    let chat_path = tmp.join("test.jsonl");
    {
        let mut f = std::fs::File::create(&chat_path).unwrap();
        for i in 0..25 {
            let msg = ChatMessage::new_user(format!("History item {}", i));
            writeln!(f, "{}", serde_json::to_string(&msg).unwrap()).unwrap();
        }
    }
    app.check_chat_log_updates();

    let resolutions = [
        ("Compact (80x24)", 80, 24),
        ("Standard (160x50)", 160, 50),
        ("Ultra-Wide (240x70)", 240, 70),
    ];

    for (res_label, width, height) in resolutions {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        for active_agents in [1, 2, 4, 6] {
            let mut frame_latencies = Vec::with_capacity(100);
            for frame in 0..100 {
                for i in 0..active_agents {
                    let chunk = generate_ansi_chunk(frame * 10 + i);
                    app.clients[i].vt_parser.lock().unwrap().process(&chunk);
                }
                let start = Instant::now();
                app.check_chat_log_updates();
                terminal.draw(|f| ui::render(f, &mut app)).unwrap();
                frame_latencies.push(start.elapsed().as_nanos() as f64 / 1_000_000.0);
            }
            let stats = compute_stats(frame_latencies);
            println!(
                "  Viewport: {:18} | {:1} Agents: Mean: {:5.2} ms | p50: {:5.2} ms | p95: {:5.2} ms | Jitter(σ): {:4.2} ms | Headroom: {:4.1}%",
                res_label, active_agents, stats.mean, stats.p50, stats.p95, stats.std_dev, ((16.666 - stats.mean) / 16.666) * 100.0
            );
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);

    println!("\n=== Benchmark Complete ===");
}
