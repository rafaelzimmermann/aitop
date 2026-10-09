#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub label: String,
    pub pct: f64,
    pub detail: String,
    pub pace: Option<String>,
    /// cap in use for this window, as displayed (e.g. "200.0k", "$30.00")
    pub cap: Option<String>,
}

impl Row {
    pub fn new(label: &str, pct: f64, detail: String) -> Row {
        Row {
            label: label.to_string(),
            pct,
            detail,
            pace: None,
            cap: None,
        }
    }

    pub fn set_cap(&mut self, cap: &str) {
        self.cap = Some(cap.to_string());
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    // per-request estimates are often sub-cent; don't collapse them to $0.00
    if n > 0.0 && n < 0.01 {
        format!("${n:.4}")
    } else {
        format!("${n:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_scale_with_size() {
        assert_eq!(fmt_duration(45), "45s");
        assert_eq!(fmt_duration(120), "2m");
        assert_eq!(fmt_duration(9000), "2h30m");
        assert_eq!(fmt_duration(300000), "3d11h");
    }

    #[test]
    fn window_labels() {
        assert_eq!(window_label(0), "window");
        assert_eq!(window_label(1800), "30m window");
        assert_eq!(window_label(18000), "5h window");
        assert_eq!(window_label(86400), "24h window");
        assert_eq!(window_label(604800), "7d window");
        assert_eq!(window_label(2592000), "4w window");
    }

    #[test]
    fn number_formatting() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1500), "1.5k");
        assert_eq!(fmt_tokens(2_000_000), "2.00M");
        assert_eq!(fmt_money(12.34), "$12.34");
        assert_eq!(fmt_money(0.000123), "$0.0001");
    }

    #[test]
    fn rows_start_without_pace() {
        let r = Row::new("daily", 10.0, "x".into());
        assert_eq!(r.pace, None);
        assert_eq!(r.cap, None);
        let p = Panel::new("codex");
        assert_eq!(p.source, None);
        assert!(!p.stale);
    }
}
