mod config;
mod history;
mod local;
mod model;
mod pace;
mod pricing;
mod providers;
mod ui;
mod util;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, backend::TestBackend, layout::Position, Terminal};

use crate::config::Config;
use crate::model::{ascii_bar, fmt_tokens, Snapshot};
use crate::pricing::Pricing;

const HELP: &str = "aitop — htop for AI usage\n\n\
usage: aitop [--json] [--plain] [--watch N] [--redact] [--history] [--help]\n\n\
  TUI keys: q quit · r refresh now · h help · 1-9 focus provider · tab cycle · Enter zoom · j/k scroll\n\n\
providers:\n\
  codex      GET {CODEX_BASE_URL}/codex/usage (OAuth token from CODEX_AUTH_FILE)\n\
  claude     GET {ANTHROPIC_BASE_URL}/api/oauth/usage + local session logs\n\
  copilot    GET {GITHUB_API_BASE_URL}/copilot_internal/user (GITHUB_TOKEN)\n\
  z.ai       no public quota API → local accounting from PI_SESSION_DIR vs ZAI_LIMIT_*\n\
  openrouter GET {OPENROUTER_BASE_URL}/key + /credits\n\
  other      no quota API → local session logs (totals + output tok/s)\n\n\
--plain/--json print one snapshot; add --watch N to keep refreshing every N seconds\n--redact hides the account email and API key prefixes (useful when piping --json to a file)\n--history draws the sparkline over 7 daily buckets instead of 24 hourly ones\n";

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// One char per hourly bucket, scaled to the tallest bucket.
fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn sparkline(data: &[u64]) -> String {
    let max = data.iter().max().copied().unwrap_or(1).max(1) as f64;
    data.iter()
        .map(|v| BLOCKS[(((*v as f64 / max) * 7.0).round() as usize).min(7)])
        .collect()
}

/// `--watch N` keeps the plain/json output running instead of exiting after one shot.
fn watch_secs(args: &[String]) -> Option<u64> {
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--watch=") {
            return v.trim().parse().ok();
        }
        if a == "--watch" {
            return args.get(i + 1).and_then(|v| v.trim().parse().ok());
        }
    }
    None
}

/// Reload the pricing map only when the cached copy has gone stale, so a long-running
/// TUI does not keep the startup snapshot forever.
fn refresh_pricing(cfg: &Config, pricing: &Pricing) -> Pricing {
    if pricing.is_stale(cfg.pricing_max_age_hours) {
        pricing::load(
            &cfg.cache_dir,
            &cfg.openrouter_base,
            cfg.pricing_max_age_hours,
        )
    } else {
        pricing.clone()
    }
}

fn print_plain(snap: &Snapshot) {
    println!("aitop · {}", snap.fetched_at);
    for p in &snap.panels {
        println!("\n{}  {}", p.name, p.subtitle);
        for r in &p.rows {
            let pace = r
                .pace
                .as_deref()
                .map(|p| format!(" · {p}"))
                .unwrap_or_default();
            println!(
                "  {:<14} {:>5.0}%  {}  {}{}",
                r.label,
                r.pct,
                ascii_bar(r.pct, 24),
                r.detail,
                pace
            );
        }
        if !p.spark.is_empty() {
            println!(
                "  {:<12} {:>5}   {}",
                p.spark_label,
                fmt_tokens(p.spark.iter().sum()),
                sparkline(&p.spark)
            );
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }

    let mut cfg = config::load();
    if has_flag(&args, "--redact") {
        cfg.redact = true;
    }
    if has_flag(&args, "--history") {
        cfg.history = true;
    }
    let mut pricing = pricing::load(
        &cfg.cache_dir,
        &cfg.openrouter_base,
        cfg.pricing_max_age_hours,
    );
    if has_flag(&args, "--json") {
        loop {
            let snap = providers::fetch_all(&cfg, &pricing);
            println!("{}", serde_json::to_string_pretty(&snap)?);
            let Some(secs) = watch_secs(&args) else { break };
            pricing = refresh_pricing(&cfg, &pricing);
            thread::sleep(Duration::from_secs(secs));
        }
        return Ok(());
    }
    if has_flag(&args, "--plain") || has_flag(&args, "-p") {
        loop {
            print_plain(&providers::fetch_all(&cfg, &pricing));
            let Some(secs) = watch_secs(&args) else { break };
            pricing = refresh_pricing(&cfg, &pricing);
            println!();
            thread::sleep(Duration::from_secs(secs));
        }
        return Ok(());
    }

    if has_flag(&args, "--render-test") {
        let snap = providers::fetch_all(&cfg, &pricing);
        let state = ui::State {
            snapshot: snap,
            focus: 0,
            help: false,
            refresh_secs: cfg.refresh_secs,
            scroll: 0,
            zoom: false,
        };
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
        snapshot: providers::fetch_all(&cfg, &pricing),
        focus: 0,
        help: false,
        refresh_secs: cfg.refresh_secs,
        scroll: 0,
        zoom: false,
    };

    let force = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    {
        let cfg = cfg.clone();
        let force = force.clone();
        thread::spawn(move || {
            let mut pricing = pricing;
            loop {
                let snap = providers::fetch_all(&cfg, &pricing);
                let _ = tx.send(snap);
                pricing = refresh_pricing(&cfg, &pricing);
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(cfg.refresh_secs.max(1)) {
                    if force.load(Ordering::Relaxed) {
                        force.store(false, Ordering::Relaxed);
                        break;
                    }
                    thread::sleep(Duration::from_millis(150));
                }
            }
        });
    }

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    // A panic while unwinding would otherwise leave the terminal in raw mode with
    // the cursor hidden; restore it before the default hook prints the report.
    std::panic::set_hook(Box::new(|info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
        eprintln!("panic: {info}");
    }));

    let result = run_loop(&mut terminal, &mut state, &force, &rx);

    std::panic::set_hook(Box::new(|info| eprintln!("panic: {info}")));
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
                    KeyCode::Esc => {
                        if state.zoom {
                            state.zoom = false;
                            state.scroll = 0;
                        } else {
                            break;
                        }
                    }
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('r') => force.store(true, Ordering::Relaxed),
                    KeyCode::Char('h') => state.help = !state.help,
                    KeyCode::Enter => {
                        state.zoom = !state.zoom;
                        state.scroll = 0;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if state.zoom {
                            state.scroll = state.scroll.saturating_add(1)
                        } else {
                            state.focus = (state.focus + 1) % count
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if state.zoom {
                            state.scroll = state.scroll.saturating_sub(1)
                        } else {
                            state.focus = (state.focus + count - 1) % count
                        }
                    }
                    KeyCode::Tab => state.focus = (state.focus + 1) % count,
                    KeyCode::BackTab => state.focus = (state.focus + count - 1) % count,
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        let i = (c as usize) - ('1' as usize);
                        if i < count {
                            state.focus = i;
                            state.scroll = 0;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_scales_to_the_tallest_bucket() {
        assert_eq!(sparkline(&[0, 1, 7]), "▁▂█");
        assert_eq!(sparkline(&[10, 10]), "██");
        assert_eq!(sparkline(&[]), "");
    }

    #[test]
    fn flags_are_optional() {
        let args = ["--plain", "--history"].map(String::from);
        assert!(has_flag(&args, "--history"));
        assert!(!has_flag(&args, "--redact"));
    }

    #[test]
    fn watch_flag_is_optional_and_parsed() {
        assert_eq!(watch_secs(&["--plain".to_string()]), None);
        assert_eq!(
            watch_secs(&["--plain", "--watch", "30"].map(String::from)),
            Some(30)
        );
        assert_eq!(watch_secs(&["--watch=5".to_string()]), Some(5));
        assert_eq!(watch_secs(&["--watch", "nope"].map(String::from)), None);
    }

    #[test]
    fn fresh_pricing_is_reused_without_a_reload() {
        let cfg = config::test_config();
        let fresh = pricing::Pricing {
            fetched_at: Some(chrono::Utc::now().to_rfc3339()),
            source: "cache".into(),
            ..Default::default()
        };
        assert!(!fresh.is_stale(cfg.pricing_max_age_hours));
        assert_eq!(refresh_pricing(&cfg, &fresh).source, "cache");
    }
}
