use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Sparkline},
    Frame,
};

use crate::model::{fmt_tokens, Panel, Snapshot};

pub struct State {
    pub snapshot: Snapshot,
    pub focus: usize,
    pub help: bool,
    pub refresh_secs: u64,
}

fn bar_color(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::Red
    } else if pct >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn draw_panel(f: &mut Frame, area: Rect, p: &Panel, focused: bool) {
    let border = if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .title(p.name.as_str())
        .title_style(
            Style::default().fg(if focused { Color::Cyan } else { Color::White }).add_modifier(Modifier::BOLD),
        )
        .border_style(border);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 {
        return;
    }

    let nrows = p.rows.len().max(1);
    let mut constraints = vec![Constraint::Length(1)];
    for _ in 0..nrows {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Min(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    f.render_widget(
        Paragraph::new(p.subtitle.clone()).style(Style::default().fg(Color::DarkGray)),
        chunks[0],
    );

    for (i, row) in p.rows.iter().enumerate() {
        let gauge = Gauge::default()
            .ratio((row.pct / 100.0).clamp(0.0, 1.0))
            .gauge_style(Style::default().fg(bar_color(row.pct)).bg(Color::Black))
            .label(format!("{:<14} {:>5.0}%  {}", row.label, row.pct, row.detail));
        f.render_widget(gauge, chunks[1 + i]);
    }

    let spark_area = chunks[1 + nrows];
    if !p.spark.is_empty() {
        f.render_widget(
            Sparkline::default().data(&p.spark).style(Style::default().fg(Color::Blue)),
            spark_area,
        );
    }

    let lines: Vec<Line> = p
        .error
        .iter()
        .map(|e| Line::from(Span::styled(format!("⚠ {e}"), Style::default().fg(Color::Red))))
        .chain(p.lines.iter().map(|l| Line::from(l.clone())))
        .collect();
    f.render_widget(Paragraph::new(lines).style(Style::default().fg(Color::Gray)), chunks[2 + nrows]);
}

pub fn draw(f: &mut Frame, s: &State) {
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(if s.help { 6 } else { 1 }),
        ])
        .split(size);

    // tab bar
    let mut tabline = Line::default();
    for (i, p) in s.snapshot.panels.iter().enumerate() {
        let style = if i == s.focus {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        tabline.push_span(Span::styled(format!(" {} {} ", i + 1, p.name), style));
    }
    tabline.push_span(Span::styled(
        format!(" · {}", s.snapshot.fetched_at),
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(tabline), chunks[0]);

    let n = s.snapshot.panels.len().max(1) as u16;
    let per = (chunks[1].height / n).max(3);
    for (i, p) in s.snapshot.panels.iter().enumerate() {
        let area = Rect {
            x: chunks[1].x,
            y: chunks[1].y + (i as u16) * per,
            width: chunks[1].width,
            height: per,
        };
        draw_panel(f, area, p, i == s.focus);
    }

    if s.help {
        f.render_widget(Block::new().borders(Borders::ALL).title("help").border_style(Style::default().fg(Color::Cyan)), chunks[2]);
        let body = [
            "q quit · r refresh now · h toggle help · 1-9 focus provider · tab/↑/↓ cycle",
            "codex      live quota via chatgpt.com/backend-api/codex/usage",
            "z.ai       no public quota API → local accounting from pi session logs",
            "openrouter live credits/usage via /key + /credits",
        ];
        for (i, line) in body.iter().enumerate() {
            let a = Rect {
                x: chunks[2].x + 1,
                y: chunks[2].y + 1 + i as u16,
                width: chunks[2].width.saturating_sub(2),
                height: 1,
            };
            f.render_widget(Paragraph::new(Line::from(line.to_string())), a);
        }
    } else {
        let total: u64 = s.snapshot.panels.iter().map(|p| p.spark.iter().sum::<u64>()).sum();
        let footer = format!(
            "q quit · r refresh · h help · focus: {} · refresh {}s · 24h tokens {}",
            s.snapshot.panels.get(s.focus).map(|p| p.name.clone()).unwrap_or_default(),
            s.refresh_secs,
            fmt_tokens(total)
        );
        f.render_widget(Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)), chunks[2]);
    }
}
