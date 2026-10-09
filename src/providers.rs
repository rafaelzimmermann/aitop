use std::io::Write;

use chrono::{Datelike, DateTime, Timelike, Utc};
use serde_json::Value;

use crate::config::{self, Config};
use crate::local;
use crate::model::{fmt_duration, fmt_money, fmt_tokens, Panel, Row, Snapshot, window_label};
use crate::pace;
use crate::pricing::Pricing;

const UA: &str = "aitop/0.1 (+https://github.com/; htop-for-ai-usage)";

#[derive(Debug)]
pub struct FetchErr {
    pub status: Option<u16>,
    pub text: String,
}

impl std::fmt::Display for FetchErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text)
    }
}

impl From<ureq::Error> for FetchErr {
    fn from(e: ureq::Error) -> FetchErr {
        match &e {
            ureq::Error::Status(code, _) => FetchErr {
                status: Some(*code),
                text: format!("HTTP {code}"),
            },
            other => FetchErr { status: None, text: other.to_string() },
        }
    }
}

fn request(url: &str, key: Option<&str>, extra: &[(&str, String)], body: Option<&Value>) -> Result<ureq::Response, FetchErr> {
    let mut req = if body.is_some() { ureq::post(url) } else { ureq::get(url) };
    req = req.set("User-Agent", UA).set("accept", "application/json");
    if let Some(k) = key {
        req = req.set("Authorization", &format!("Bearer {k}"));
    }
    for (h, v) in extra {
        req = req.set(h, v);
    }
    let resp = match body {
        Some(b) => req.set("content-type", "application/json").send_json(b.clone()),
        None => req.call(),
    };
    resp.map_err(FetchErr::from)
}

fn get_json(url: &str, key: Option<&str>, extra: &[(&str, String)]) -> Result<Value, FetchErr> {
    let resp = request(url, key, extra, None)?;
    resp.into_json::<Value>().map_err(|e| FetchErr { status: None, text: e.to_string() })
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

fn add_row(p: &mut Panel, cfg: &Config, label: String, pct: f64, detail: String, window_secs: u64, reset_after: u64) {
    let mut r = Row::new(&label, pct, detail);
    if let Some(pa) = pace::assess(window_secs, reset_after, pct, cfg.pace_trigger) {
        r.pace = Some(pace::label(&pa));
    }
    p.rows.push(r);
}

fn add_local_rows(p: &mut Panel, cfg: &Config, stats: &local::Stats) {
    let l = &cfg.zai_limits;
    let pct = |used: u64, limit: u64| if limit > 0 { (used as f64 / limit as f64) * 100.0 } else { 0.0 };
    let elapsed = |start: &Option<DateTime<Utc>>| start.map(|t| (Utc::now() - t).num_seconds().max(0) as u64).unwrap_or(0);

    let mut r = Row::new("5h window", pct(stats.tokens_5h, l.five_hour), format!("{} / {}", fmt_tokens(stats.tokens_5h), fmt_tokens(l.five_hour)));
    if let Some(pa) = pace::assess_elapsed(elapsed(&stats.first_5h), 5 * 3600, r.pct, cfg.pace_trigger) {
        r.pace = Some(pace::label(&pa));
    }
    p.rows.push(r);

    let mut r = Row::new("daily", pct(stats.tokens_24h, l.day), format!("{} / {}", fmt_tokens(stats.tokens_24h), fmt_tokens(l.day)));
    if let Some(pa) = pace::assess_elapsed(elapsed(&stats.first_24h), 24 * 3600, r.pct, cfg.pace_trigger) {
        r.pace = Some(pace::label(&pa));
    }
    p.rows.push(r);

    let mut r = Row::new("weekly", pct(stats.tokens_7d, l.week), format!("{} / {}", fmt_tokens(stats.tokens_7d), fmt_tokens(l.week)));
    if let Some(pa) = pace::assess_elapsed(elapsed(&stats.first_7d), 7 * 86400, r.pct, cfg.pace_trigger) {
        r.pace = Some(pace::label(&pa));
    }
    p.rows.push(r);

    let rpm = stats.requests_5h as f64 / 5.0;
    p.rows.push(Row::new("avg rpm", pct(rpm as u64, l.rpm), format!("{:.1} / {}", rpm, l.rpm)));
}

fn add_local_lines(p: &mut Panel, stats: &local::Stats) {
    p.lines.push(format!(
        "requests: {} total · {} in last 5h",
        stats.requests, stats.requests_5h
    ));
    if stats.cost_total > 0.0 {
        p.lines.push(format!("est. cost: {} total · {} in 24h", fmt_money(stats.cost_total), fmt_money(stats.cost_24h)));
    }
    for m in stats.models.iter().take(3) {
        p.lines.push(format!(
            "  {:<22} {} · {} req{}",
            m.model,
            fmt_tokens(m.tokens),
            m.requests,
            if m.cost > 0.0 { format!(" · {}", fmt_money(m.cost)) } else { String::new() }
        ));
    }
    if let Some(last) = &stats.last_request {
        p.lines.push(format!("last request: {last}"));
    }
    p.spark = stats.spark.clone();
}

// ---------------------------------------------------------------- Codex

/// Cached refreshed token (we never rewrite codex's own auth.json).
fn cached_token(cfg: &Config) -> Option<String> {
    let raw = std::fs::read_to_string(cfg.cache_dir.join("codex_token.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let expires = v.get("expires_at").and_then(|x| x.as_i64()).unwrap_or(0);
    if expires <= Utc::now().timestamp() {
        return None;
    }
    v.get("access_token").and_then(|x| x.as_str()).map(str::to_string)
}

fn refresh_codex_token(cfg: &Config) -> Option<String> {
    let rt = config::codex_refresh_token(cfg)?;
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": cfg.codex_client_id,
        "scope": "openid profile email",
        "responses_type": "token",
        "refresh_token": rt,
    });
    let url = format!("{}/oauth/token", cfg.auth_base.trim_end_matches('/'));
    let resp = request(&url, None, &[], Some(&body)).ok()?;
    let v = resp.into_json::<Value>().ok()?;
    let token = v.get("access_token").and_then(|x| x.as_str())?;
    let ttl = v.get("expires_in").and_then(|x| x.as_i64()).unwrap_or(3600);
    let out = serde_json::json!({ "access_token": token, "expires_at": Utc::now().timestamp() + ttl });
    if let Ok(raw) = serde_json::to_string(&out) {
        let _ = std::fs::create_dir_all(&cfg.cache_dir);
        let path = cfg.cache_dir.join("codex_token.json");
        if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(&path) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fh.metadata().map(|m| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)));
            }
            let _ = fh.write_all(raw.as_bytes());
        }
    }
    Some(token.to_string())
}

fn codex_panel(cfg: &Config, pricing: &Pricing, v: &Value) -> Panel {
    let mut p = Panel::new("codex");
    p.source = Some("live quota API + local rollout logs".into());
    p.subtitle = format!(
        "{} · {}",
        v.get("plan_type").and_then(|x| x.as_str()).unwrap_or("unknown plan"),
        v.get("email").and_then(|x| x.as_str()).unwrap_or("no email")
    );

    if let Some(rl) = v.get("rate_limit") {
        for key in ["primary_window", "secondary_window"] {
            let w = match rl.get(key) {
                Some(w) if !w.is_null() => w,
                _ => continue,
            };
            let secs = w.get("limit_window_seconds").and_then(|x| x.as_u64()).unwrap_or(0);
            let pct = w.get("used_percent").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let reset = w.get("reset_after_seconds").and_then(|x| x.as_u64()).unwrap_or(0);
            let detail = if reset > 0 { format!("resets in {}", fmt_duration(reset)) } else { "no reset".into() };
            add_row(&mut p, cfg, window_label(secs), pct, detail, secs, reset);
        }
        if rl.get("limit_reached").and_then(|x| x.as_bool()).unwrap_or(false) {
            p.lines.push("⚠ rate limit reached".to_string());
        }
    }

    if let Some(extra) = v.get("additional_rate_limits").and_then(|a| a.as_array()) {
        for item in extra {
            let rl = match item.get("rate_limit") {
                Some(r) => r,
                None => continue,
            };
            let w = match rl.get("primary_window") {
                Some(w) if !w.is_null() => w,
                _ => continue,
            };
            let secs = w.get("limit_window_seconds").and_then(|x| x.as_u64()).unwrap_or(0);
            let pct = w.get("used_percent").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let reset = w.get("reset_after_seconds").and_then(|x| x.as_u64()).unwrap_or(0);
            let name = item.get("limit_name").and_then(|x| x.as_str()).unwrap_or("extra");
            add_row(&mut p, cfg, format!("{name} {}", window_label(secs)), pct, format!("resets in {}", fmt_duration(reset)), secs, reset);
        }
    }

    let credits = v.get("credits");
    let balance = credits
        .and_then(|c| c.get("balance"))
        .and_then(|b| b.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| b.as_f64()))
        .unwrap_or(0.0);
    let has_credits = credits.and_then(|c| c.get("has_credits")).and_then(|b| b.as_bool()).unwrap_or(false);
    let unlimited = credits.and_then(|c| c.get("unlimited")).and_then(|b| b.as_bool()).unwrap_or(false);
    p.lines.push(format!(
        "credits: {}",
        if unlimited {
            "unlimited".to_string()
        } else if has_credits {
            fmt_money(balance)
        } else {
            "none".to_string()
        }
    ));
    if let Some(rc) = v.get("rate_limit_reset_credits") {
        let avail = rc.get("available_count").and_then(|x| x.as_u64()).unwrap_or(0);
        if avail > 0 {
            p.lines.push(format!("reset credits available: {avail}"));
        }
    }

    let stats = local::codex_usage(&cfg.codex_session_dir, pricing);
    p.lines.push(format!(
        "local logs: {} tokens / {} requests (24h {})",
        fmt_tokens(stats.total),
        stats.requests,
        fmt_tokens(stats.tokens_24h)
    ));
    add_local_lines(&mut p, &stats);
    p
}

pub fn codex(cfg: &Config, pricing: &Pricing) -> Panel {
    let mut p = Panel::new("codex");
    let token = match cached_token(cfg).or_else(|| config::codex_access_token(cfg)) {
        Some(t) => t,
        None => {
            p.error = Some("no codex token (set CODEX_ACCESS_TOKEN or CODEX_AUTH_FILE)".into());
            return p;
        }
    };
    let url = format!("{}/codex/usage", cfg.codex_base.trim_end_matches('/'));
    let extra = cfg
        .codex_installation_id
        .as_ref()
        .map(|i| vec![("x-codex-installation-id", i.clone())])
        .unwrap_or_default();

    match get_json(&url, Some(&token), &extra) {
        Ok(v) => codex_panel(cfg, pricing, &v),
        Err(e) => {
            // expired access token → try the refresh token once
            if e.status == Some(401) {
                if let Some(fresh) = refresh_codex_token(cfg) {
                    if let Ok(v) = get_json(&url, Some(&fresh), &extra) {
                        return codex_panel(cfg, pricing, &v);
                    }
                }
            }
            p.error = Some(format!("{url} -> {e}"));
            p
        }
    }
}

// ---------------------------------------------------------------- Claude

pub fn claude(cfg: &Config, pricing: &Pricing) -> Panel {
    let mut p = Panel::new("claude");
    p.source = Some("anthropic oauth usage API + local session logs".into());

    let raw = match std::fs::read_to_string(&cfg.claude_credentials_file) {
        Ok(r) => r,
        Err(_) => {
            p.error = Some(format!("no credentials at {}", cfg.claude_credentials_file.display()));
            return p;
        }
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            p.error = Some("could not parse claude credentials".into());
            return p;
        }
    };
    let oauth = v.get("claudeAiOauth").or_else(|| v.get("claude_ai_oauth"));
    let token = oauth.and_then(|o| o.get("accessToken").or_else(|| o.get("access_token")).and_then(|x| x.as_str())).unwrap_or("");
    if token.is_empty() {
        p.error = Some("no claude access token".into());
        return p;
    }
    let tier = oauth.and_then(|o| o.get("rateLimitTier").or_else(|| o.get("rate_limit_tier")).and_then(|x| x.as_str())).unwrap_or("unknown tier");
    let expires = oauth
        .and_then(|o| o.get("expiresAt").or_else(|| o.get("expires_at")).and_then(|x| x.as_f64()))
        .map(|ms| DateTime::from_timestamp_millis(ms as i64));
    if let Some(e) = expires {
        p.subtitle = format!("{tier} · token expires {}", e.to_utc().to_rfc3339());
        if e.to_utc() <= Utc::now() {
            p.lines.push("⚠ token expired — run `claude` to refresh".to_string());
        }
    } else {
        p.subtitle = tier.to_string();
    }

    let url = format!("{}/api/oauth/usage", cfg.anthropic_base.trim_end_matches('/'));
    match get_json(&url, Some(token), &[("anthropic-beta", "oauth-2025-04-20".to_string())]) {
        Ok(u) => {
            for (key, window) in [("five_hour", 18000u64), ("seven_day", 604800u64), ("seven_day_oauth_apps", 604800u64)] {
                let w = match u.get(key) {
                    Some(w) if w.is_object() => w,
                    _ => continue,
                };
                let pct = w.get("utilization").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let reset = match w.get("resets_at").and_then(|x| x.as_str()).and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
                    Some(t) => (t.to_utc() - Utc::now()).num_seconds().max(0) as u64,
                    None => 0,
                };
                add_row(&mut p, cfg, window_label(window), pct, format!("resets in {}", fmt_duration(reset)), window, reset);
            }
            if let Some(o) = u.get("additional_utility").and_then(|x| x.as_bool()) {
                p.lines.push(format!("additional usage: {o}"));
            }
        }
        Err(e) => p.error = Some(format!("{url} -> {e}")),
    }

    let stats = local::pi_usage(&cfg.pi_session_dir, "anthropic", pricing);
    if stats.requests > 0 {
        p.lines.push(format!("local logs: {} tokens / {} requests", fmt_tokens(stats.total), stats.requests));
        add_local_lines(&mut p, &stats);
    }
    p
}

// ---------------------------------------------------------------- Copilot

pub fn copilot(cfg: &Config, _pricing: &Pricing) -> Panel {
    let mut p = Panel::new("copilot");
    p.source = Some("github copilot quota API".into());
    let token = match &cfg.github_token {
        Some(t) => t.clone(),
        None => {
            p.error = Some("no github token (set GITHUB_TOKEN or use `gh auth login`)".into());
            return p;
        }
    };

    let url = format!("{}/copilot_internal/user", cfg.github_base.trim_end_matches('/'));
    let v = match get_json(&url, Some(&token), &[]) {
        Ok(v) => v,
        Err(e) => {
            p.error = Some(format!("{url} -> {e}"));
            return p;
        }
    };

    let sku = v.get("access_type_sku").and_then(|x| x.as_str()).unwrap_or("unknown");
    p.subtitle = format!("plan: {sku}");

    let now = Utc::now();
    let month_end = {
        let (y, m) = if now.month() == 12 { (now.year() + 1, 1) } else { (now.year(), now.month() + 1) };
        chrono::DateTime::parse_from_rfc3339(&format!("{y:04}-{m:02}-01T00:00:00Z")).map(|t| t.to_utc()).ok()
    };
    let reset = month_end.map(|t| (t - now).num_seconds().max(0) as u64).unwrap_or(0);

    let snapshots = v.get("quota_snapshots").or_else(|| v.get("quotaSnapshots"));
    let mut shown = 0;
    if let Some(s) = snapshots.and_then(|x| x.as_object()) {
        for (name, q) in s {
            let pct = q.get("percent_remaining").and_then(|x| x.as_f64());
            let remaining = q.get("remaining").and_then(|x| x.as_f64());
            let entitlement = q.get("entitlement").and_then(|x| x.as_f64());
            if pct.is_none() && remaining.is_none() {
                continue;
            }
            let pct = pct.unwrap_or_else(|| {
                let e = entitlement.unwrap_or(0.0);
                if e > 0.0 { ((e - remaining.unwrap_or(0.0)) / e) * 100.0 } else { 0.0 }
            });
            let used = 100.0 - pct;
            let detail = format!(
                "{} left of {}",
                fmt_tokens(remaining.unwrap_or(0.0) as u64),
                fmt_tokens(entitlement.unwrap_or(0.0) as u64)
            );
            add_row(&mut p, cfg, format!("{name} (month)"), used, detail, 30 * 86400, reset);
            shown += 1;
            if shown >= 3 {
                break;
            }
        }
    }
    if shown == 0 {
        p.lines.push("no quota snapshot in response".to_string());
    }
    p
}

// ---------------------------------------------------------------- z.ai

pub fn zai(cfg: &Config, pricing: &Pricing) -> Panel {
    let mut p = Panel::new("z.ai");
    p.source = Some("local accounting (no public quota API)".into());
    p.subtitle = match &cfg.zai_key {
        Some(k) => format!("key {}… · local accounting", &k[..8.min(k.len())]),
        None => "no key (set ZAI_API_KEY)".to_string(),
    };

    let stats = local::pi_usage(&cfg.pi_session_dir, "zai", pricing);
    add_local_rows(&mut p, cfg, &stats);

    if let Some(k) = &cfg.zai_key {
        let url = format!("{}/models", cfg.zai_base.trim_end_matches('/'));
        match request(&url, Some(k), &[], None) {
            Ok(resp) => {
                for h in resp.headers_names() {
                    if h.contains("ratelimit") {
                        if let Some(v) = resp.header(&h) {
                            p.lines.push(format!("{h}: {v}"));
                        }
                    }
                }
                if let Ok(j) = resp.into_json::<Value>() {
                    let n = j.get("data").and_then(|d| d.as_array()).map(|a| a.len()).unwrap_or(0);
                    p.lines.push(format!("key valid · {n} models available"));
                }
            }
            Err(e) => p.lines.push(format!("live probe failed: {e}")),
        }
    }

    add_local_lines(&mut p, &stats);
    p
}

// ---------------------------------------------------------------- OpenRouter

pub fn openrouter(cfg: &Config, pricing: &Pricing) -> Panel {
    let mut p = Panel::new("openrouter");
    p.source = Some("live key + credits API".into());
    let key = match &cfg.openrouter_key {
        Some(k) => k.clone(),
        None => {
            p.error = Some("no key (set OPENROUTER_API_KEY)".into());
            return p;
        }
    };
    let base = cfg.openrouter_base.trim_end_matches('/');

    let keyinfo = match get_json(&format!("{base}/key"), Some(&key), &[]) {
        Ok(v) => v,
        Err(e) => {
            p.error = Some(format!("{base}/key -> {e}"));
            return p;
        }
    };
    let credits = get_json(&format!("{base}/credits"), Some(&key), &[]).ok();

    let d = keyinfo.get("data").unwrap_or(&keyinfo);
    p.subtitle = d.get("label").and_then(|x| x.as_str()).unwrap_or("api key").to_string();

    let total_credits = credits.as_ref().and_then(|c| c.get("data")).and_then(|c| c.get("total_credits")).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let usage = num(d, "usage").unwrap_or(0.0);
    let pct = if total_credits > 0.0 { (usage / total_credits) * 100.0 } else { 0.0 };
    p.rows.push(Row::new("credits", pct, format!("{} used of {}", fmt_money(usage), fmt_money(total_credits))));

    if let Some(lim) = num(d, "limit") {
        let remaining = num(d, "limit_remaining").unwrap_or(0.0);
        p.rows.push(Row::new(
            "key limit",
            if lim > 0.0 { ((lim - remaining) / lim) * 100.0 } else { 0.0 },
            format!("{} left of {}", fmt_money(remaining), fmt_money(lim)),
        ));
    }

    let now = Utc::now();
    let day_elapsed = (now.num_seconds_from_midnight() as u64);
    let week_elapsed = day_elapsed + (now.weekday().num_days_from_monday() as u64) * 86400;
    let month_elapsed = day_elapsed + (now.day0() as u64) * 86400;

    for (label, key_name, window, elapsed) in [
        ("daily", "usage_daily", 86400u64, day_elapsed),
        ("weekly", "usage_weekly", 7 * 86400, week_elapsed),
        ("monthly", "usage_monthly", 30 * 86400, month_elapsed),
    ] {
        let v = num(d, key_name).unwrap_or(0.0);
        let mut r = Row::new(label, if total_credits > 0.0 { (v / total_credits) * 100.0 } else { 0.0 }, fmt_money(v));
        if total_credits > 0.0 {
            if let Some(pa) = pace::assess_elapsed(elapsed, window, r.pct, cfg.pace_trigger) {
                r.pace = Some(pace::label(&pa));
            }
        }
        p.rows.push(r);
    }

    p.lines.push(format!(
        "tier: {} · management key: {}",
        if d.get("is_free_tier").and_then(|x| x.as_bool()).unwrap_or(false) { "free" } else { "paid" },
        d.get("is_management_key").and_then(|x| x.as_bool()).unwrap_or(false)
    ));
    if !pricing.is_empty() {
        p.lines.push(format!("pricing cache: {} models · {}", pricing.len(), pricing.source));
    }
    p
}

// ---------------------------------------------------------------- all

pub fn fetch_all(cfg: &Config, pricing: &Pricing) -> Snapshot {
    let mut panels = Vec::new();
    for name in &cfg.providers {
        match name.as_str() {
            "codex" => panels.push(codex(cfg, pricing)),
            "claude" => panels.push(claude(cfg, pricing)),
            "copilot" => panels.push(copilot(cfg, pricing)),
            "z.ai" | "zai" => panels.push(zai(cfg, pricing)),
            "openrouter" => panels.push(openrouter(cfg, pricing)),
            other => {
                let mut p = Panel::new(other);
                p.error = Some("unknown provider".into());
                panels.push(p);
            }
        }
    }
    Snapshot {
        fetched_at: Utc::now().to_rfc3339(),
        panels,
    }
}
