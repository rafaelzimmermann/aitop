mod config;
mod local;
mod model;
mod providers;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, backend::TestBackend, layout::Position, Terminal};

use crate::model::{fmt_tokens, Snapshot};

const HELP: &str = "aitop — htop for AI usage\n\n\
usage: aitop [--json] [--plain] [--help]\n\n\
  TUI keys: q quit · r refresh now · h help · 1-9 focus provider · tab/↑/↓ cycle\n\n\
providers:\n\
  codex      GET {CODEX_BASE_URL}/codex/usage (OAuth token from CODEX_AUTH_FILE)\n\
  z.ai       no public quota API → local accounting from PI_SESSION_DIR vs ZAI_LIMIT_*\n\
  openrouter GET {OPENROUTER_BASE_URL}/key + /credits\n";

fn ascii_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0).clamp(0.0, 1.0) * width as f64).round() as usize;
    let mut s = String::new();
    for i in 0..width {
        s.push(if i < filled { '█' } else { '░' });
    }
    s
}

fn print_plain(snap: &Snapshot) {
    println!("aitop · {}", snap.fetched_at);
    for p in &snap.panels {
        println!("\n{}  {}", p.name, p.subtitle);
        for r in &p.rows {
            println!(
                "  {:<14} {:>5.0}%  {}  {}",
                r.label,
                r.pct,
                ascii_bar(r.pct, 24),
                r.detail
            );
        }
        if !p.spark.is_empty() {
            let max = p.spark.iter().max().copied().unwrap_or(1).max(1);
            let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
            let line: String = p.spark.iter().map(|v| blocks[(((*v as f64 / max as f64) * 7.0).round() as usize).min(7)]).collect();
            println!("  24h tokens   {:>5}   {}", fmt_tokens(p.spark.iter().sum()), line);
        }
        for l in &p.lines {
            println!("  {l}");
        }
        if let Some(e) = &p.error {
            println!("  ⚠ {e}");
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = config::load();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--json") {
        let snap = providers::fetch_all(&cfg);
        println!("{}", serde_json::to_string_pretty(&snap)?);
        return Ok(());
    }
    if args.iter().any(|a| a == "--plain" || a == "-p") {
        print_plain(&providers::fetch_all(&cfg));
        return Ok(());
    }

    if args.iter().any(|a| a == "--render-test") {
        let snap = providers::fetch_all(&cfg);
        let state = ui::State { snapshot: snap, focus: 0, help: false, refresh_secs: cfg.refresh_secs };
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|f| {
            ui::draw(f, &state);
            let area = f.area();
            let buf = f.buffer_mut();
            for y in 0..area.height {
                let mut line = String::new();
                for x in 0..area.width {
                    line.push_str(buf[Position::new(x, y)].symbol());
                }
                println!("{line}");
            }
        })?;
        return Ok(());
    }

    let mut state = ui::State {
        snapshot: providers::fetch_all(&cfg),
        focus: 0,
        help: false,
        refresh_secs: cfg.refresh_secs,
    };

    let force = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    {
        let cfg = cfg.clone();
        let force = force.clone();
        thread::spawn(move || loop {
            let snap = providers::fetch_all(&cfg);
            let _ = tx.send(snap);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(cfg.refresh_secs.max(1)) {
                if force.load(Ordering::Relaxed) {
                    force.store(false, Ordering::Relaxed);
                    break;
                }
                thread::sleep(Duration::from_millis(150));
            }
        });
    }

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = run_loop(&mut terminal, &mut state, &force, &rx);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut ui::State,
    force: &AtomicBool,
    rx: &mpsc::Receiver<Snapshot>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| ui::draw(f, state))?;
        if let Ok(s) = rx.try_recv() {
            state.snapshot = s;
        }
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                let count = state.snapshot.panels.len().max(1);
                match k.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Esc => break,
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('r') => force.store(true, Ordering::Relaxed),
                    KeyCode::Char('h') => state.help = !state.help,
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        let i = (c as usize) - ('1' as usize);
                        if i < count {
                            state.focus = i;
                        }
                    }
                    KeyCode::Tab | KeyCode::Down => state.focus = (state.focus + 1) % count,
                    KeyCode::BackTab | KeyCode::Up => state.focus = (state.focus + count - 1) % count,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
