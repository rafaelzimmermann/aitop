#[derive(Debug, Clone, serde::Serialize)]
pub struct Row {
    pub label: String,
    pub pct: f64,
    pub detail: String,
    pub pace: Option<String>,
}

impl Row {
    pub fn new(label: &str, pct: f64, detail: String) -> Row {
        Row {
            label: label.to_string(),
            pct,
            detail,
            pace: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Panel {
    pub name: String,
    pub subtitle: String,
    pub rows: Vec<Row>,
    pub lines: Vec<String>,
    pub spark: Vec<u64>,
    pub error: Option<String>,
    /// true when the values shown are the last good ones, not a live fetch
    pub stale: bool,
    /// where the numbers come from (API endpoint, local logs, ...)
    pub source: Option<String>,
}

impl Panel {
    pub fn new(name: &str) -> Panel {
        Panel {
            name: name.to_string(),
            subtitle: String::new(),
            rows: Vec::new(),
            lines: Vec::new(),
            spark: Vec::new(),
            error: None,
            stale: false,
            source: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Snapshot {
    pub fetched_at: String,
    pub panels: Vec<Panel>,
}

pub fn fmt_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        return format!("{h}h{m:02}m");
    }
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    format!("{d}d{h:02}h")
}

pub fn window_label(secs: u64) -> String {
    match secs {
        0 => "window".to_string(),
        s if s <= 3600 => format!("{}m window", s / 60),
        s if s <= 86400 => format!("{}h window", s / 3600),
        s if s <= 604800 => format!("{}d window", s / 86400),
        s => format!("{}w window", s / 604800),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

pub fn fmt_money(n: f64) -> String {
    format!("${n:.2}")
}
