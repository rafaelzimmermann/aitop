use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Sparkline},
    Frame,
};

use crate::model::{bar_parts, fmt_tokens, Panel, Row, Snapshot};

pub struct State {
    pub snapshot: Snapshot,
    pub focus: usize,
    pub help: bool,
    pub refresh_secs: u64,
    /// scroll offset (in lines) applied to the focused panel's detail lines
    pub scroll: usize,
    /// true while the focused panel is expanded to the full viewport
    pub zoom: bool,
}

// fixed columns keep every bar starting at the same column, whatever the label length is
const LABEL_W: usize = 14;
const PCT_W: usize = 5;
const BAR_W: usize = 24;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const TEXT: Color = Color::Gray;

/// terminals at least this wide get panels tiled into two columns
const TWO_COL_MIN_WIDTH: usize = 110;

fn bar_color(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::Red
    } else if pct >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn pace_color(p: &str) -> Color {
    if p.contains("ahead") {
        Color::Green
    } else if p.contains("behind") {
        Color::Yellow
    } else {
        MUTED
    }
}

/// Clean wall-clock form of an RFC3339 timestamp: `13:00:16 UTC` (falls back to the
/// raw string when it does not parse).
fn clock(rfc: &str) -> String {
    if let Some(dot) = rfc.find('.') {
        if rfc.len() >= 19 {
            let hms = &rfc[11..dot];
            let tz = &rfc[dot..];
            if tz.ends_with("+00:00") || tz.ends_with("-00:00") || tz.ends_with('Z') {
                return format!("{hms} UTC");
            }
            return hms.to_string();
        }
    }
    if rfc.len() >= 20 && rfc[19..].ends_with('Z') {
        return format!("{} UTC", &rfc[11..19]);
    }
    if rfc.len() >= 25 && (rfc[19..].ends_with("+00:00") || rfc[19..].ends_with("-00:00")) {
        return format!("{} UTC", &rfc[11..19]);
    }
    if rfc.len() >= 19 && rfc.chars().nth(10) == Some('T') {
        return rfc[11..19].to_string();
    }
    rfc.to_string()
}

fn fit(s: &str, width: usize) -> String {
    s.chars().take(width).collect()
}

/// label · pct · bar · detail, each in its own fixed column
fn row_line(row: &Row, width: usize) -> Line<'_> {
    let bar_w = BAR_W.min(width.saturating_sub(LABEL_W + PCT_W + 2 + 1));
    let detail_w = width.saturating_sub(LABEL_W + PCT_W + 2 + bar_w + 1);
    let (filled, empty) = bar_parts(row.pct, bar_w);
    let color = bar_color(row.pct);

    let mut spans = vec![
        Span::styled(
            format!("{:<LABEL_W$}", fit(&row.label, LABEL_W)),
            Style::default().fg(TEXT),
        ),
        Span::styled(
            format!("{:>PCT_W$}", format!("{:.0}%", row.pct)),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
    ];
    for _ in 0..filled {
        spans.push(Span::styled(
            "█",
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    for _ in 0..empty {
        spans.push(Span::styled("░", Style::default().fg(MUTED)));
    }
    spans.push(Span::styled(
        format!(" {}", fit(&row.detail, detail_w)),
        Style::default().fg(TEXT),
    ));
    if let Some(p) = &row.pace {
        spans.push(Span::styled(
            format!(" · {p}"),
            Style::default().fg(pace_color(p)),
        ));
    }
    Line::from(spans)
}

fn panel_title(p: &Panel, width: usize) -> String {
    let mut t = p.name.clone();
    if p.stale {
        t.push_str(" stale");
    }
    if let Some(s) = &p.source {
        let room = width.saturating_sub(t.chars().count() + 3);
        if room >= 8 {
            t.push_str(&format!(" · {}", s.chars().take(room).collect::<String>()));
        }
    }
    t
}

fn draw_panel(f: &mut Frame, area: Rect, p: &Panel, focused: bool, scroll: usize) {
    let border = if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    };
    let title_style = Style::default()
        .fg(if p.stale {
            MUTED
        } else if focused {
            ACCENT
        } else {
            Color::White
        })
        .add_modifier(Modifier::BOLD);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(panel_title(p, area.width as usize))
        .title_style(title_style)
        .border_style(border);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 {
        return;
    }

    let mut constraints = vec![Constraint::Length(1)];
    for _ in 0..p.rows.len() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Min(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    f.render_widget(
        Paragraph::new(p.subtitle.clone()).style(Style::default().fg(MUTED)),
        chunks[0],
    );

    for (i, row) in p.rows.iter().enumerate() {
        f.render_widget(
            Paragraph::new(row_line(row, inner.width as usize)),
            chunks[1 + i],
        );
    }

    let spark_area = chunks[1 + p.rows.len()];
    if !p.spark.is_empty() {
        let left = Rect {
            x: spark_area.x,
            y: spark_area.y,
            width: (LABEL_W + PCT_W + 2).min(spark_area.width as usize) as u16,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(format!(
                "{:<LABEL_W$}{:>PCT_W$}",
                fit(&p.spark_label, LABEL_W),
                fmt_tokens(p.spark.iter().sum())
            )))
            .style(Style::default().fg(TEXT)),
            left,
        );
        let right = Rect {
            x: left.x + left.width,
            y: spark_area.y,
            width: spark_area.width.saturating_sub(left.width),
            height: 1,
        };
        if right.width > 0 {
            f.render_widget(
                Sparkline::default()
                    .data(&p.spark)
                    .style(Style::default().fg(ACCENT)),
                right,
            );
        }
    }

    // detail lines: the focused panel scrolls with `scroll`; others show what fits
    let lines: Vec<Line> = p
        .error
        .iter()
        .map(|e| {
            Line::from(Span::styled(
                format!("⚠ {e}"),
                Style::default().fg(Color::Red),
            ))
        })
        .chain(
            p.lines
                .iter()
                .map(|l| Line::from(l.clone()).style(Style::default().fg(TEXT))),
        )
        .collect();
    let area_lines = chunks[2 + p.rows.len()];
    let visible = lines.len().saturating_sub(scroll);
    f.render_widget(
        Paragraph::new(lines.iter().skip(scroll).cloned().collect::<Vec<Line>>()),
        area_lines,
    );
    if focused && visible < lines.len() {
        // scrollbar hints: how many lines are hidden above and below
        let hidden = lines.len() - visible;
        let hint = format!("▲ {} above · ▼ {} more", scroll.min(999), hidden.min(999));
        let y = area_lines.y + area_lines.height.saturating_sub(1);
        if area_lines.height > 0 {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    fit(&hint, area_lines.width as usize),
                    Style::default().fg(MUTED),
                ))),
                Rect {
                    x: area_lines.x,
                    y,
                    width: area_lines.width,
                    height: 1,
                },
            );
        }
    }
}

pub fn draw(f: &mut Frame, s: &State) {
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(if s.help { 11 } else { 1 }),
        ])
        .split(size);

    // tab bar
    let mut tabline = Line::default();
    for (i, p) in s.snapshot.panels.iter().enumerate() {
        let style = if i == s.focus {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        tabline.push_span(Span::styled(format!(" {} {} ", i + 1, p.name), style));
    }
    tabline.push_span(Span::styled(
        format!(" · {}", clock(&s.snapshot.fetched_at)),
        Style::default().fg(MUTED),
    ));
    f.render_widget(Paragraph::new(tabline), chunks[0]);

    if s.zoom {
        // focused panel expanded to the whole content area
        if let Some(p) = s.snapshot.panels.get(s.focus) {
            draw_panel(f, chunks[1], p, true, s.scroll);
        }
    } else if s.snapshot.panels.len() >= 2 && size.width as usize >= TWO_COL_MIN_WIDTH {
        // wide terminals: two columns halve the vertical starvation per panel
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[1]);
        let n = s.snapshot.panels.len();
        let left_n = n.div_ceil(2);
        // uniform height across both columns so their bottom edges align;
        // the taller column defines the row height, leftover rows stay blank
        let per = (cols[0].height / left_n as u16).max(3);
        for (col, lo, hi) in [(0, 0, left_n), (1, left_n, n)] {
            for (j, i) in (lo..hi).enumerate() {
                let p = &s.snapshot.panels[i];
                let area = Rect {
                    x: cols[col].x,
                    y: cols[col].y + (j as u16) * per,
                    width: cols[col].width,
                    height: per,
                };
                draw_panel(
                    f,
                    area,
                    p,
                    i == s.focus,
                    if i == s.focus { s.scroll } else { 0 },
                );
            }
        }
    } else {
        let n = s.snapshot.panels.len().max(1) as u16;
        let per = (chunks[1].height / n).max(3);
        for (i, p) in s.snapshot.panels.iter().enumerate() {
            let area = Rect {
                x: chunks[1].x,
                y: chunks[1].y + (i as u16) * per,
                width: chunks[1].width,
                height: per,
            };
            draw_panel(
                f,
                area,
                p,
                i == s.focus,
                if i == s.focus { s.scroll } else { 0 },
            );
        }
    }

    if s.help {
        f.render_widget(
            Block::new()
                .borders(Borders::ALL)
                .title("help")
                .border_style(Style::default().fg(ACCENT)),
            chunks[2],
        );
        let body = [
            "q quit · r refresh now · h toggle help · 1-9 focus provider · tab/↑/↓ cycle",
            "Enter zoom focused panel · j/k or ↑/↓ scroll it while zoomed · Esc back",
            "codex      live quota via chatgpt.com/backend-api/codex/usage",
            "claude     live utilization via api.anthropic.com/api/oauth/usage",
            "copilot    live quota via api.github.com/copilot_internal/user",
            "z.ai       live quota via api.z.ai/api/monitor/usage/quota/limit (fallback: local logs)",
            "openrouter live credits/usage via /key + /credits",
            "other      any other name → local session logs, output tok/s from parent gaps",
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
        let total: u64 = s
            .snapshot
            .panels
            .iter()
            .map(|p| p.spark.iter().sum::<u64>())
            .sum();
        let footer = format!(
            "q quit · r refresh · h help · Enter zoom · j/k scroll · focus: {} · refresh {}s · 24h tokens {}",
            s.snapshot
                .panels
                .get(s.focus)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            s.refresh_secs,
            fmt_tokens(total)
        );
        f.render_widget(
            Paragraph::new(footer).style(Style::default().fg(MUTED)),
            chunks[2],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_color_thresholds() {
        assert_eq!(bar_color(69.0), Color::Green);
        assert_eq!(bar_color(70.0), Color::Yellow);
        assert_eq!(bar_color(89.9), Color::Yellow);
        assert_eq!(bar_color(90.0), Color::Red);
    }

    #[test]
    fn pace_colors_follow_the_state() {
        assert_eq!(pace_color("pace ahead 38%"), Color::Green);
        assert_eq!(pace_color("pace behind 20%"), Color::Yellow);
        assert_eq!(pace_color("pace on track"), MUTED);
    }

    #[test]
    fn fit_truncates_to_the_column() {
        assert_eq!(fit("weekly 7d window", 14), "weekly 7d wind");
        assert_eq!(fit("5h", 14), "5h");
    }

    #[test]
    fn rows_use_fixed_columns_so_bars_line_up() {
        let long = Row::new("weekly 7d window", 44.0, "44% used".into());
        let short = Row::new("5h", 3.0, "resets in 2h30m".into());
        let a = row_line(&long, 60);
        let b = row_line(&short, 60);

        let cells = |l: &Line| -> (usize, usize, usize) {
            let label = l.spans[0].content.chars().count();
            let pct = l.spans[1].content.chars().count();
            let bar = l.spans[3..3 + BAR_W]
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>();
            (label, pct, bar)
        };
        assert_eq!(cells(&a), (LABEL_W, PCT_W, BAR_W));
        assert_eq!(cells(&b), (LABEL_W, PCT_W, BAR_W));
        // same number of spans → the detail starts at the same column for every row
        assert_eq!(a.spans.len(), b.spans.len());
        assert_eq!(a.spans[3].style.fg, Some(bar_color(44.0)));
        assert_eq!(b.spans[3].style.fg, Some(bar_color(3.0)));
        // long labels are truncated instead of pushing the bar right
        assert_eq!(a.spans[0].content, "weekly 7d wind");
        assert_eq!(b.spans[0].content, "5h            ");
    }

    #[test]
    fn clock_strips_the_nanosecond_tail() {
        assert_eq!(clock("2026-10-09T13:00:16.675005046+00:00"), "13:00:16 UTC");
        assert_eq!(clock("2026-10-09T13:00:16Z"), "13:00:16 UTC");
        assert_eq!(clock("2026-10-09T13:00:16+02:00"), "13:00:16");
        assert_eq!(clock("not-a-timestamp"), "not-a-timestamp");
    }
}
