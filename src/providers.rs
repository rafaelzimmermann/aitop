use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::Value;

use crate::config::{self, Config, Limits};
use crate::history;
use crate::local;
use crate::model::{fmt_duration, fmt_money, fmt_tokens, window_label, Panel, Row, Snapshot};
use crate::pace;
use crate::pricing::Pricing;
use crate::util;

const UA: &str = "aitop/0.1 (+https://github.com/; htop-for-ai-usage)";

/// A hung provider must not stall a refresh tick, so every request is bounded.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Only the first few characters of a key ever reach the screen / --json.
fn mask_key(cfg: &Config, k: &str) -> String {
    if cfg.redact {
        return "[redacted]".to_string();
    }
    let taken: String = k.chars().take(4).collect();
    if k.chars().count() > taken.chars().count() {
        format!("{taken}…")
    } else {
        taken
    }
}

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
            other => FetchErr {
                status: None,
                text: other.to_string(),
            },
        }
    }
}

fn request(
    url: &str,
    key: Option<&str>,
    extra: &[(&str, String)],
    body: Option<&Value>,
) -> Result<ureq::Response, FetchErr> {
    let mut req = if body.is_some() {
        ureq::post(url)
    } else {
        ureq::get(url)
    };
    req = req
        .timeout(TIMEOUT)
        .set("User-Agent", UA)
        .set("accept", "application/json");
    if let Some(k) = key {
        req = req.set("Authorization", &format!("Bearer {k}"));
    }
    for (h, v) in extra {
        req = req.set(h, v);
    }
    let resp = match body {
        Some(b) => req
            .set("content-type", "application/json")
            .send_json(b.clone()),
        None => req.call(),
    };
    resp.map_err(FetchErr::from)
}

fn get_json(url: &str, key: Option<&str>, extra: &[(&str, String)]) -> Result<Value, FetchErr> {
    let resp = request(url, key, extra, None)?;
    resp.into_json::<Value>().map_err(|e| FetchErr {
        status: None,
        text: e.to_string(),
    })
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

fn add_row(p: &mut Panel, cfg: &Config, mut r: Row, window_secs: u64, reset_after: u64) {
    if let Some(pa) = pace::assess(window_secs, reset_after, r.pct, cfg.pace_trigger) {
        r.pace = Some(pace::label(&pa));
    }
    p.rows.push(r);
}

fn add_local_rows(p: &mut Panel, cfg: &Config, stats: &local::Stats, l: &Limits) {
    let pct = |used: u64, limit: u64| {
        if limit > 0 {
            (used as f64 / limit as f64) * 100.0
        } else {
            0.0
        }
    };
    let elapsed = |start: &Option<DateTime<Utc>>| {
        start
            .map(|t| (Utc::now() - t).num_seconds().max(0) as u64)
            .unwrap_or(0)
    };

    // a row with no cap is still useful: it shows the measured value against the pace
    let mut push =
        |label: &str, used: u64, limit: u64, window_secs: u64, start: &Option<DateTime<Utc>>| {
            let mut r = Row::new(
                label,
                pct(used, limit),
                format!("{} / {}", fmt_tokens(used), fmt_tokens(limit)),
            );
            if limit > 0 {
                r.set_cap(&fmt_tokens(limit));
            }
            // pace only means something against a cap
            if limit > 0 {
                if let Some(pa) =
                    pace::assess_elapsed(elapsed(start), window_secs, r.pct, cfg.pace_trigger)
                {
                    r.pace = Some(pace::label(&pa));
                }
            }
            p.rows.push(r);
        };

    push(
        "5h window",
        stats.tokens_5h,
        l.five_hour,
        5 * 3600,
        &stats.first_5h,
    );
    push(
        "daily",
        stats.tokens_24h,
        l.day,
        24 * 3600,
        &stats.first_24h,
    );
    push(
        "weekly",
        stats.tokens_7d,
        l.week,
        7 * 86400,
        &stats.first_7d,
    );

    let rpm = stats.requests_5h as f64 / 5.0;
    let mut r = Row::new(
        "avg rpm",
        pct(rpm as u64, l.rpm),
        format!("{:.1} / {}", rpm, l.rpm),
    );
    if l.rpm > 0 {
        r.set_cap(&l.rpm.to_string());
    }
    p.rows.push(r);
}

fn add_local_lines(p: &mut Panel, cfg: &Config, stats: &local::Stats) {
    p.lines.push(format!(
        "requests: {} total · {} in last 5h",
        stats.requests, stats.requests_5h
    ));
    if let Some(t) = stats.tps_24h {
        let last = stats
            .last_tps
            .map(|v| format!(" · last {:.1}", v))
            .unwrap_or_default();
        p.lines
            .push(format!("output tok/s: {:.1} (24h mean){}", t, last));
    }
    if stats.cost_total > 0.0 {
        p.lines.push(format!(
            "est. cost: {} total · {} in 24h",
            fmt_money(stats.cost_total),
            fmt_money(stats.cost_24h)
        ));
    }
    for m in stats.models.iter().take(3) {
        let tps = m
            .tps
            .map(|v| format!(" · {:.0} tok/s", v))
            .unwrap_or_default();
        p.lines.push(format!(
            "  {:<22} {} · {} req{}{}",
            m.model,
            fmt_tokens(m.tokens),
            m.requests,
            if m.cost > 0.0 {
                format!(" · {}", fmt_money(m.cost))
            } else {
                String::new()
            },
            tps
        ));
    }
    if let Some(last) = &stats.last_request {
        p.lines.push(format!("last request: {last}"));
    }
    if cfg.history {
        p.spark = stats.daily.clone();
        p.spark_label = "7d tokens".to_string();
    } else {
        p.spark = stats.spark.clone();
        p.spark_label = "24h tokens".to_string();
    }
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
    v.get("access_token")
        .and_then(|x| x.as_str())
        .map(str::to_string)
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
    let out =
        serde_json::json!({ "access_token": token, "expires_at": Utc::now().timestamp() + ttl });
    if let Ok(raw) = serde_json::to_string(&out) {
        util::secret_dir(&cfg.cache_dir);
        util::write_secret(&cfg.cache_dir.join("codex_token.json"), &raw);
    }
    Some(token.to_string())
}

fn codex_panel(cfg: &Config, pricing: &Pricing, v: &Value) -> Panel {
    let mut p = Panel::new("codex");
    p.source = Some("live quota API + local rollout logs".into());
    let plan = v
        .get("plan_type")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown plan");
    let email = match v.get("email").and_then(|x| x.as_str()) {
        Some(e) if !cfg.redact => e,
        _ => "[redacted]",
    };
    p.subtitle = format!("{plan} · {email}");

    if let Some(rl) = v.get("rate_limit") {
        for key in ["primary_window", "secondary_window"] {
            let w = match rl.get(key) {
                Some(w) if !w.is_null() => w,
                _ => continue,
            };
            let secs = w
                .get("limit_window_seconds")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let pct = w
                .get("used_percent")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0);
            let reset = w
                .get("reset_after_seconds")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let detail = if reset > 0 {
                format!("resets in {}", fmt_duration(reset))
            } else {
                "no reset".into()
            };
            add_row(
                &mut p,
                cfg,
                Row::new(&window_label(secs), pct, detail),
                secs,
                reset,
            );
        }
        if rl
            .get("limit_reached")
            .and_then(|x| x.as_bool())
            .unwrap_or(false)
        {
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
            let secs = w
                .get("limit_window_seconds")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let pct = w
                .get("used_percent")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0);
            let reset = w
                .get("reset_after_seconds")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let name = item
                .get("limit_name")
                .and_then(|x| x.as_str())
                .unwrap_or("extra");
            let r = Row::new(
                &format!("{name} {}", window_label(secs)),
                pct,
                format!("resets in {}", fmt_duration(reset)),
            );
            add_row(&mut p, cfg, r, secs, reset);
        }
    }

    let credits = v.get("credits");
    let balance = credits
        .and_then(|c| c.get("balance"))
        .and_then(|b| {
            b.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| b.as_f64())
        })
        .unwrap_or(0.0);
    let has_credits = credits
        .and_then(|c| c.get("has_credits"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let unlimited = credits
        .and_then(|c| c.get("unlimited"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
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
        let avail = rc
            .get("available_count")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
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
    add_local_lines(&mut p, cfg, &stats);
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

fn claude_panel(
    cfg: &Config,
    pricing: &Pricing,
    oauth: &Value,
    usage: Option<&Value>,
    error: Option<String>,
) -> Panel {
    let mut p = Panel::new("claude");
    p.source = Some("anthropic oauth usage API + local session logs".into());
    if let Some(e) = error {
        p.error = Some(e);
        return p;
    }

    let tier = oauth
        .get("rateLimitTier")
        .or_else(|| oauth.get("rate_limit_tier"))
        .and_then(|x| x.as_str())
        .unwrap_or("unknown tier");
    let expires = oauth
        .get("expiresAt")
        .or_else(|| oauth.get("expires_at"))
        .and_then(|x| x.as_f64())
        .and_then(|ms| DateTime::from_timestamp_millis(ms as i64));
    if let Some(e) = expires {
        p.subtitle = format!("{tier} · token expires {}", e.to_rfc3339());
        if e <= Utc::now() {
            p.lines
                .push("⚠ token expired — run `claude` to refresh".to_string());
        }
    } else {
        p.subtitle = tier.to_string();
    }

    if let Some(u) = usage {
        for (key, window) in [
            ("five_hour", 18000u64),
            ("seven_day", 604800u64),
            ("seven_day_oauth_apps", 604800u64),
        ] {
            let w = match u.get(key) {
                Some(w) if w.is_object() => w,
                _ => continue,
            };
            let pct = w.get("utilization").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let reset = match w
                .get("resets_at")
                .and_then(|x| x.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            {
                Some(t) => (t.to_utc() - Utc::now()).num_seconds().max(0) as u64,
                None => 0,
            };
            add_row(
                &mut p,
                cfg,
                Row::new(
                    &window_label(window),
                    pct,
                    format!("resets in {}", fmt_duration(reset)),
                ),
                window,
                reset,
            );
        }
        if let Some(o) = u.get("additional_utility").and_then(|x| x.as_bool()) {
            p.lines.push(format!("additional usage: {o}"));
        }
    }

    let stats = local::pi_usage(&cfg.pi_session_dir, "anthropic", pricing);
    if stats.requests > 0 {
        p.lines.push(format!(
            "local logs: {} tokens / {} requests",
            fmt_tokens(stats.total),
            stats.requests
        ));
        add_local_lines(&mut p, cfg, &stats);
    }
    p
}

pub fn claude(cfg: &Config, pricing: &Pricing) -> Panel {
    let raw = match std::fs::read_to_string(&cfg.claude_credentials_file) {
        Ok(r) => r,
        Err(_) => {
            return claude_panel(
                cfg,
                pricing,
                &Value::Null,
                None,
                Some(format!(
                    "no credentials at {}",
                    cfg.claude_credentials_file.display()
                )),
            )
        }
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            return claude_panel(
                cfg,
                pricing,
                &Value::Null,
                None,
                Some("could not parse claude credentials".into()),
            )
        }
    };
    let oauth = v
        .get("claudeAiOauth")
        .or_else(|| v.get("claude_ai_oauth"))
        .unwrap_or(&Value::Null);
    let token = oauth
        .get("accessToken")
        .or_else(|| oauth.get("access_token"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if token.is_empty() {
        return claude_panel(
            cfg,
            pricing,
            oauth,
            None,
            Some("no claude access token".into()),
        );
    }
    let url = format!(
        "{}/api/oauth/usage",
        cfg.anthropic_base.trim_end_matches('/')
    );
    match get_json(
        &url,
        Some(token),
        &[("anthropic-beta", "oauth-2025-04-20".to_string())],
    ) {
        Ok(u) => claude_panel(cfg, pricing, oauth, Some(&u), None),
        Err(e) => claude_panel(cfg, pricing, oauth, None, Some(format!("{url} -> {e}"))),
    }
}

// ---------------------------------------------------------------- Copilot

fn copilot_panel(cfg: &Config, v: &Value, error: Option<String>) -> Panel {
    let mut p = Panel::new("copilot");
    p.source = Some("github copilot quota API".into());
    if let Some(e) = error {
        p.error = Some(e);
        return p;
    }

    let sku = v
        .get("access_type_sku")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown");
    p.subtitle = format!("plan: {sku}");

    let now = Utc::now();
    let month_end = {
        let (y, m) = if now.month() == 12 {
            (now.year() + 1, 1)
        } else {
            (now.year(), now.month() + 1)
        };
        chrono::DateTime::parse_from_rfc3339(&format!("{y:04}-{m:02}-01T00:00:00Z"))
            .map(|t| t.to_utc())
            .ok()
    };
    let reset = month_end
        .map(|t| (t - now).num_seconds().max(0) as u64)
        .unwrap_or(0);

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
            // percent_remaining is what's left; remaining/entitlement is already what's used
            let used = match pct {
                Some(p) => 100.0 - p,
                None => {
                    let e = entitlement.unwrap_or(0.0);
                    if e > 0.0 {
                        ((e - remaining.unwrap_or(0.0)) / e) * 100.0
                    } else {
                        0.0
                    }
                }
            };
            let detail = format!(
                "{} left of {}",
                fmt_tokens(remaining.unwrap_or(0.0) as u64),
                fmt_tokens(entitlement.unwrap_or(0.0) as u64)
            );
            let mut r = Row::new(&format!("{name} (month)"), used, detail);
            if let Some(e) = entitlement {
                r.set_cap(&fmt_tokens(e as u64));
            }
            add_row(&mut p, cfg, r, 30 * 86400, reset);
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

pub fn copilot(cfg: &Config, _pricing: &Pricing) -> Panel {
    let token = match &cfg.github_token {
        Some(t) => t.clone(),
        None => {
            return copilot_panel(
                cfg,
                &Value::Null,
                Some("no github token (set GITHUB_TOKEN or use `gh auth login`)".into()),
            )
        }
    };
    let url = format!(
        "{}/copilot_internal/user",
        cfg.github_base.trim_end_matches('/')
    );
    match get_json(&url, Some(&token), &[]) {
        Ok(v) => copilot_panel(cfg, &v, None),
        Err(e) => copilot_panel(cfg, &Value::Null, Some(format!("{url} -> {e}"))),
    }
}

// ---------------------------------------------------------------- z.ai

fn zai_panel(cfg: &Config, stats: &local::Stats, probe_lines: &[String]) -> Panel {
    let mut p = Panel::new("z.ai");
    p.source = Some("local accounting (no public quota API)".into());
    p.subtitle = match &cfg.zai_key {
        Some(k) => format!("key {} · local accounting", mask_key(cfg, k)),
        None => "no key (set ZAI_API_KEY)".to_string(),
    };

    // when the gateway sends x-ratelimit-* headers they supersede the ZAI_LIMIT_* guesses
    let live = live_limits(probe_lines);
    let caps = Limits {
        five_hour: if live.five_hour > 0 {
            live.five_hour
        } else {
            cfg.zai_limits.five_hour
        },
        day: if live.day > 0 {
            live.day
        } else {
            cfg.zai_limits.day
        },
        week: if live.week > 0 {
            live.week
        } else {
            cfg.zai_limits.week
        },
        month: if live.month > 0 {
            live.month
        } else {
            cfg.zai_limits.month
        },
        rpm: if live.rpm > 0 {
            live.rpm
        } else {
            cfg.zai_limits.rpm
        },
    };
    add_local_rows(&mut p, cfg, stats, &caps);
    for l in probe_lines {
        p.lines.push(l.clone());
    }
    add_local_lines(&mut p, cfg, stats);
    p
}

/// Caps derived from live `x-ratelimit-*` response headers (zero where absent).
/// Header names seen in the wild: `x-ratelimit-limit`, `x-ratelimit-limit-day`,
/// `x-ratelimit-remaining`, `x-ratelimit-reset`. Checked 2026-10-09: api.z.ai sends
/// none, so this is a probe that only takes effect if the gateway starts sending them.
fn live_limits(probe_lines: &[String]) -> Limits {
    let get = |key: &str| -> u64 {
        for l in probe_lines {
            let pre = format!("{key}: ");
            if l.starts_with(&pre) {
                return l[pre.len()..].trim().parse().unwrap_or(0);
            }
        }
        0
    };
    Limits {
        five_hour: get("x-ratelimit-limit-5h"),
        day: get("x-ratelimit-limit-day"),
        week: get("x-ratelimit-limit-week"),
        month: get("x-ratelimit-limit-month"),
        rpm: get("x-ratelimit-limit"),
    }
}

/// Providers with no quota API: everything comes from the local session logs. Bars only
/// appear when a cap is configured for them, otherwise the panel is totals + throughput.
fn local_panel(cfg: &Config, name: &str, stats: &local::Stats) -> Panel {
    let mut p = Panel::new(name);
    p.source = Some("local accounting (no public quota API)".into());
    p.subtitle = "session logs".to_string();
    add_local_rows(&mut p, cfg, stats, &Limits::default());
    add_local_lines(&mut p, cfg, stats);
    p
}

pub fn zai(cfg: &Config, pricing: &Pricing) -> Panel {
    let stats = local::pi_usage(&cfg.pi_session_dir, "zai", pricing);
    let mut probe_lines = Vec::new();
    if let Some(k) = &cfg.zai_key {
        let url = format!("{}/models", cfg.zai_base.trim_end_matches('/'));
        match request(&url, Some(k), &[], None) {
            Ok(resp) => {
                for h in resp.headers_names() {
                    if h.contains("ratelimit") {
                        if let Some(v) = resp.header(&h) {
                            probe_lines.push(format!("{h}: {v}"));
                        }
                    }
                }
                if let Ok(j) = resp.into_json::<Value>() {
                    let n = j
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    probe_lines.push(format!("key valid · {n} models available"));
                }
            }
            Err(e) => probe_lines.push(format!("live probe failed: {e}")),
        }
    }
    zai_panel(cfg, &stats, &probe_lines)
}

// ---------------------------------------------------------------- OpenRouter

fn openrouter_panel(
    cfg: &Config,
    pricing: &Pricing,
    d: &Value,
    credits: Option<&Value>,
    error: Option<String>,
) -> Panel {
    let mut p = Panel::new("openrouter");
    p.source = Some("live key + credits API".into());
    if let Some(e) = error {
        p.error = Some(e);
        return p;
    }
    p.subtitle = d
        .get("label")
        .and_then(|x| x.as_str())
        .unwrap_or("api key")
        .to_string();

    let total_credits = credits
        .and_then(|c| c.get("data"))
        .and_then(|c| c.get("total_credits"))
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0);
    let usage = num(d, "usage").unwrap_or(0.0);
    let pct = if total_credits > 0.0 {
        (usage / total_credits) * 100.0
    } else {
        0.0
    };
    let mut credits_row = Row::new(
        "credits",
        pct,
        format!("{} used of {}", fmt_money(usage), fmt_money(total_credits)),
    );
    if total_credits > 0.0 {
        credits_row.set_cap(&fmt_money(total_credits));
    }
    p.rows.push(credits_row);

    if let Some(lim) = num(d, "limit") {
        let remaining = num(d, "limit_remaining").unwrap_or(0.0);
        let mut r = Row::new(
            "key limit",
            if lim > 0.0 {
                ((lim - remaining) / lim) * 100.0
            } else {
                0.0
            },
            format!("{} left of {}", fmt_money(remaining), fmt_money(lim)),
        );
        if lim > 0.0 {
            r.set_cap(&fmt_money(lim));
        }
        p.rows.push(r);
    }

    let now = Utc::now();
    let day_elapsed = now.num_seconds_from_midnight() as u64;
    let week_elapsed = day_elapsed + (now.weekday().num_days_from_monday() as u64) * 86400;
    let month_elapsed = day_elapsed + (now.day0() as u64) * 86400;

    for (label, key_name, window, elapsed, budget) in [
        (
            "daily",
            "usage_daily",
            86400u64,
            day_elapsed,
            cfg.budget.day,
        ),
        (
            "weekly",
            "usage_weekly",
            7 * 86400,
            week_elapsed,
            cfg.budget.week,
        ),
        (
            "monthly",
            "usage_monthly",
            30 * 86400,
            month_elapsed,
            cfg.budget.month,
        ),
    ] {
        let v = num(d, key_name).unwrap_or(0.0);
        let mut r = Row::new(
            label,
            if budget > 0 {
                (v / budget as f64) * 100.0
            } else if total_credits > 0.0 {
                (v / total_credits) * 100.0
            } else {
                0.0
            },
            fmt_money(v),
        );
        if budget > 0 {
            r.set_cap(&fmt_money(budget as f64));
            if let Some(pa) = pace::assess_elapsed(elapsed, window, r.pct, cfg.pace_trigger) {
                r.pace = Some(pace::label(&pa));
            }
        }
        p.rows.push(r);
    }

    p.lines.push(format!(
        "tier: {} · management key: {}",
        if d.get("is_free_tier")
            .and_then(|x| x.as_bool())
            .unwrap_or(false)
        {
            "free"
        } else {
            "paid"
        },
        d.get("is_management_key")
            .and_then(|x| x.as_bool())
            .unwrap_or(false)
    ));
    if !pricing.is_empty() {
        p.lines.push(format!(
            "pricing cache: {} models · {}",
            pricing.len(),
            pricing.source
        ));
    }
    p
}

pub fn openrouter(cfg: &Config, pricing: &Pricing) -> Panel {
    let key = match &cfg.openrouter_key {
        Some(k) => k.clone(),
        None => {
            return openrouter_panel(
                cfg,
                pricing,
                &Value::Null,
                None,
                Some("no key (set OPENROUTER_API_KEY)".into()),
            )
        }
    };
    let base = cfg.openrouter_base.trim_end_matches('/');
    let keyinfo = match get_json(&format!("{base}/key"), Some(&key), &[]) {
        Ok(v) => v,
        Err(e) => {
            return openrouter_panel(
                cfg,
                pricing,
                &Value::Null,
                None,
                Some(format!("{base}/key -> {e}")),
            )
        }
    };
    let credits = get_json(&format!("{base}/credits"), Some(&key), &[]).ok();
    let d = keyinfo.get("data").unwrap_or(&keyinfo);
    openrouter_panel(cfg, pricing, d, credits.as_ref(), None)
}

// ---------------------------------------------------------------- all

fn panel_cache_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("panel-{name}.json"))
}

fn read_panel_cache(dir: &Path, name: &str) -> Option<Panel> {
    let raw = std::fs::read_to_string(panel_cache_path(dir, name)).ok()?;
    serde_json::from_str::<Panel>(&raw).ok()
}

fn write_panel_cache(dir: &Path, p: &Panel) {
    let raw = match serde_json::to_string(p) {
        Ok(r) => r,
        Err(_) => return,
    };
    util::secret_dir(dir);
    util::write_secret(&panel_cache_path(dir, &p.name), &raw);
}

/// A failed fetch falls back to the last good panel (marked stale); a successful
/// one is persisted so the next failure can show something useful.
fn merge_cached(cfg: &Config, p: &mut Panel) {
    if p.error.is_some() {
        if let Some(cached) = read_panel_cache(&cfg.cache_dir, &p.name) {
            let err = p.error.take();
            *p = cached;
            p.stale = true;
            p.error = err;
        }
    } else {
        write_panel_cache(&cfg.cache_dir, p);
    }
}

/// Caps that changed since the last run, as one line per row.
fn cap_notes(hist: &mut history::History, p: &Panel, now: &str) -> Vec<String> {
    p.rows
        .iter()
        .filter_map(|r| {
            r.cap
                .as_ref()
                .and_then(|c| hist.note(&p.name, &r.label, c, now))
        })
        .collect()
}

fn fetch_provider(cfg: &Config, pricing: &Pricing, name: &str) -> Panel {
    match name {
        "codex" => codex(cfg, pricing),
        "claude" => claude(cfg, pricing),
        "copilot" => copilot(cfg, pricing),
        "z.ai" | "zai" => zai(cfg, pricing),
        "openrouter" => openrouter(cfg, pricing),
        other => local_panel(
            cfg,
            other,
            &local::pi_usage(&cfg.pi_session_dir, other, pricing),
        ),
    }
}

pub fn fetch_all(cfg: &Config, pricing: &Pricing) -> Snapshot {
    let mut hist = history::History::load(&cfg.cache_dir);
    let now = Utc::now().to_rfc3339();

    // Fetch concurrently: total latency is the slowest provider, not the sum.
    // Panels are collected in provider order so the display stays stable.
    let mut panels: Vec<Panel> = std::thread::scope(|s| {
        let handles: Vec<_> = cfg
            .providers
            .iter()
            .map(|name| s.spawn(move || fetch_provider(cfg, pricing, name)))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    for p in &mut panels {
        merge_cached(cfg, p);
        let notes = cap_notes(&mut hist, p, &now);
        p.lines.extend(notes);
    }
    hist.save(&cfg.cache_dir);
    Snapshot {
        fetched_at: now,
        panels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;
    use crate::pricing::Price;

    fn priced() -> Pricing {
        let mut p = Pricing {
            source: "test".into(),
            ..Default::default()
        };
        p.models.insert(
            "anthropic/claude-x".into(),
            Price {
                prompt: 1e-7,
                completion: 1e-6,
                cache_read: 0.0,
                cache_write: 0.0,
            },
        );
        p
    }

    #[test]
    fn codex_rows_and_pace() {
        let cfg = test_config();
        let v: Value = serde_json::json!({
            "plan_type": "plus",
            "email": "me@example.com",
            "rate_limit": {
                "primary_window": { "limit_window_seconds": 18000, "used_percent": 3.0, "reset_after_seconds": 9000 },
                "secondary_window": { "limit_window_seconds": 604800, "used_percent": 44.0, "reset_after_seconds": 300000 },
                "limit_reached": true
            },
            "additional_rate_limits": [
                { "limit_name": "weekly", "rate_limit": {
                    "primary_window": { "limit_window_seconds": 604800, "used_percent": 12.0, "reset_after_seconds": 300000 }
                } }
            ],
            "credits": { "has_credits": true, "unlimited": false, "balance": "12.34" },
            "rate_limit_reset_credits": { "available_count": 2 }
        });

        let p = codex_panel(&cfg, &priced(), &v);
        assert_eq!(p.subtitle, "plus · me@example.com");
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.rows[0].label, "5h window");
        assert_eq!(p.rows[0].detail, "resets in 2h30m");
        // 3% used halfway through the window → far ahead of schedule
        assert_eq!(p.rows[0].pace.as_deref(), Some("pace ahead 47%"));
        // 44% vs 50.4% expected → inside the trigger band
        assert_eq!(p.rows[1].pace.as_deref(), Some("pace on track"));
        assert_eq!(p.rows[2].label, "weekly 7d window");
        assert_eq!(p.rows[2].pace.as_deref(), Some("pace ahead 38%"));
        assert!(p.lines.iter().any(|l| l.contains("rate limit reached")));
        assert!(p.lines.iter().any(|l| l == "credits: $12.34"));
        assert!(p
            .lines
            .iter()
            .any(|l| l.contains("reset credits available: 2")));
    }

    #[test]
    fn copilot_uses_percent_remaining_and_falls_back_to_entitlement() {
        let cfg = test_config();
        let v: Value = serde_json::json!({
            "access_type_sku": "premium_plus",
            "quota_snapshots": {
                "premium_chat": { "percent_remaining": 99.0 },
                "premium_chat_extra": { "remaining": 190.0, "entitlement": 200.0 }
            }
        });
        let p = copilot_panel(&cfg, &v, None);
        assert_eq!(p.subtitle, "plan: premium_plus");
        assert_eq!(p.rows.len(), 2);
        let pct: Vec<f64> = p.rows.iter().map(|r| r.pct).collect();
        assert!(pct.contains(&1.0), "99% remaining → 1% used");
        assert!(pct.contains(&5.0), "(200-190)/200");
        assert!(p.rows.iter().all(|r| r.label.ends_with("(month)")));

        // snapshots are capped at 3 rows
        let many: Value = serde_json::json!({ "quota_snapshots": {
            "a": { "percent_remaining": 50.0 }, "b": { "percent_remaining": 50.0 },
            "c": { "percent_remaining": 50.0 }, "d": { "percent_remaining": 50.0 }
        }});
        assert_eq!(copilot_panel(&cfg, &many, None).rows.len(), 3);

        let err = copilot_panel(&cfg, &Value::Null, Some("boom".into()));
        assert_eq!(err.error.as_deref(), Some("boom"));
        assert!(err.rows.is_empty(), "an error panel shows nothing else");

        let none = copilot_panel(
            &cfg,
            &serde_json::json!({"access_type_sku": "no_access"}),
            None,
        );
        assert_eq!(none.lines.len(), 1);
        assert!(none.lines[0].contains("no quota snapshot"));
    }

    #[test]
    fn claude_tier_expiry_and_windows() {
        let cfg = test_config();
        let now = Utc::now();
        let oauth: Value = serde_json::json!({
            "rateLimitTier": "extra",
            "expiresAt": (now + chrono::Duration::hours(24)).timestamp_millis()
        });
        let usage: Value = serde_json::json!({
            "five_hour": { "utilization": 42.0, "resets_at": (now + chrono::Duration::hours(3)).to_rfc3339() },
            "seven_day_oauth_apps": { "utilization": 80.0, "resets_at": (now + chrono::Duration::days(3)).to_rfc3339() },
            "additional_utility": true
        });

        let p = claude_panel(&cfg, &priced(), &oauth, Some(&usage), None);
        assert!(p.subtitle.starts_with("extra · token expires"));
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].pct, 42.0);
        assert_eq!(p.rows[0].label, "5h window");
        // 42% used, 2/5 of the window elapsed → 40% expected → on track
        assert_eq!(p.rows[0].pace.as_deref(), Some("pace on track"));
        // 80% used, 4/7 elapsed → 57% expected → running hot
        assert_eq!(p.rows[1].pace.as_deref(), Some("pace behind 23%"));
        assert!(p.lines.iter().any(|l| l == "additional usage: true"));
        assert!(!p.lines.iter().any(|l| l.contains("token expired")));

        let expired: Value = serde_json::json!({
            "rate_limit_tier": "pro",
            "expires_at": (now - chrono::Duration::hours(1)).timestamp_millis()
        });
        let q = claude_panel(&cfg, &priced(), &expired, None, None);
        assert!(q.subtitle.starts_with("pro · token expires"));
        assert!(q.lines.iter().any(|l| l.contains("token expired")));

        let err = claude_panel(
            &cfg,
            &priced(),
            &Value::Null,
            None,
            Some("no claude access token".into()),
        );
        assert_eq!(err.error.as_deref(), Some("no claude access token"));
    }

    #[test]
    fn openrouter_credits_and_calendar_windows() {
        let mut cfg = test_config();
        cfg.budget.day = 100;
        cfg.budget.week = 200;
        cfg.budget.month = 200;
        let d: Value = serde_json::json!({
            "label": "test key",
            "usage": 100.0,
            "limit": 1000.0,
            "limit_remaining": 900.0,
            "usage_daily": 50.0,
            "usage_weekly": 100.0,
            "usage_monthly": 200.0,
            "is_free_tier": false,
            "is_management_key": true
        });
        let credits: Value = serde_json::json!({ "data": { "total_credits": 200.0 } });

        let p = openrouter_panel(&cfg, &priced(), &d, Some(&credits), None);
        assert_eq!(p.subtitle, "test key");
        assert_eq!(p.rows.len(), 5);
        assert_eq!(p.rows[0].pct, 50.0); // 100 used of 200
        assert_eq!(p.rows[1].pct, 10.0); // 100 of 1000 key limit
        assert_eq!(p.rows[2].pct, 50.0); // 50 of a 100 daily budget
        assert_eq!(p.rows[3].pct, 50.0); // 100 of a 200 weekly budget
        assert_eq!(p.rows[4].pct, 100.0); // 200 of a 200 monthly budget
        assert_eq!(p.rows[2].cap.as_deref(), Some("$100.00"));
        assert!(p
            .lines
            .iter()
            .any(|l| l.contains("tier: paid · management key: true")));
        assert!(p
            .lines
            .iter()
            .any(|l| l.contains("pricing cache: 1 models · test")));

        // without a budget, calendar rows have nothing to pace against
        let cfg2 = test_config();
        let free = openrouter_panel(&cfg2, &Pricing::default(), &d, None, None);
        assert_eq!(free.rows[0].pct, 0.0);
        assert!(free.rows.iter().all(|r| r.pace.is_none()));
        assert!(!free.lines.iter().any(|l| l.contains("pricing cache")));
    }

    #[test]
    fn zai_local_rows_compare_usage_with_elapsed_time() {
        let cfg = test_config();
        let now = Utc::now();
        let s = local::Stats {
            tokens_5h: 80_000,
            tokens_24h: 200_000,
            tokens_7d: 1_000_000,
            total: 1_000_000,
            requests: 12,
            requests_5h: 15,
            first_5h: Some(now - chrono::Duration::hours(1)),
            first_24h: Some(now - chrono::Duration::hours(12)),
            first_7d: Some(now - chrono::Duration::days(3)),
            models: vec![local::ModelStat {
                model: "glm-4.6".into(),
                requests: 12,
                tokens: 1_000_000,
                output: 0,
                secs: 0.0,
                tps: None,
                cost: 0.0,
            }],
            ..Default::default()
        };

        let p = zai_panel(&cfg, &s, &["x-ratelimit-limit: 30".to_string()]);
        assert_eq!(p.rows.len(), 4);
        assert_eq!(p.rows[0].pct, 40.0); // 80k of 200k
        assert_eq!(p.rows[0].pace.as_deref(), Some("pace behind 20%")); // 20% expected after 1h
        assert_eq!(p.rows[1].pct, 20.0);
        assert_eq!(p.rows[1].pace.as_deref(), Some("pace ahead 30%")); // 50% expected after 12h
        assert_eq!(p.rows[2].pace.as_deref(), Some("pace ahead 23%")); // 42.9% expected after 3d
        assert_eq!(p.rows[3].label, "avg rpm");
        assert_eq!(p.rows[3].detail, "3.0 / 30");
        assert!(p.lines.iter().any(|l| l.contains("x-ratelimit-limit: 30")));
        assert!(p
            .lines
            .iter()
            .any(|l| l.contains("requests: 12 total · 15 in last 5h")));
        assert!(p
            .lines
            .iter()
            .any(|l| l.contains("glm-4.6") && l.contains("1.00M") && l.contains("12 req")));
    }

    #[test]
    fn live_ratelimit_headers_override_the_configured_caps() {
        let cfg = test_config();
        let s = local::Stats {
            tokens_5h: 80_000,
            tokens_24h: 200_000,
            tokens_7d: 1_000_000,
            requests: 60,
            requests_5h: 60,
            ..Default::default()
        };
        // rpm cap comes from the live header; day cap falls back to ZAI_LIMIT_DAY
        let p = zai_panel(&cfg, &s, &["x-ratelimit-limit: 60".to_string()]);
        assert_eq!(p.rows[3].detail, "12.0 / 60");
        assert_eq!(p.rows[1].cap, Some(fmt_tokens(cfg.zai_limits.day)));
    }

    #[test]
    fn keys_are_masked_in_output() {
        let cfg = test_config();
        let mut c = cfg.clone();
        c.zai_key = Some("3af1bb111111111111111111".into());
        let p = zai_panel(&c, &local::Stats::default(), &[]);
        assert_eq!(p.subtitle, "key 3af1… · local accounting");
        assert!(!p.subtitle.contains("111111111111111111"));
    }

    #[test]
    fn unknown_providers_fall_back_to_the_local_logs() {
        let mut cfg = test_config();
        cfg.providers = vec!["nonsense".into()];
        let snap = fetch_all(&cfg, &Pricing::default());
        let p = &snap.panels[0];
        assert_eq!(p.name, "nonsense");
        assert_eq!(p.subtitle, "session logs");
        assert_eq!(p.rows.len(), 4);
        assert!(p.rows.iter().all(|r| r.cap.is_none()));
        assert!(p.lines.iter().any(|l| l.starts_with("requests: 0")));
    }

    #[test]
    fn redact_hides_identity() {
        let mut cfg = test_config();
        cfg.redact = true;
        cfg.zai_key = Some("3af1deadbeefdeadbeef".into());
        assert_eq!(
            zai_panel(&cfg, &local::Stats::default(), &[]).subtitle,
            "key [redacted] · local accounting"
        );

        let v = serde_json::json!({ "plan_type": "plus", "email": "me@example.com" });
        assert_eq!(
            codex_panel(&cfg, &priced(), &v).subtitle,
            "plus · [redacted]"
        );
    }

    #[test]
    fn last_good_panel_survives_a_failed_fetch() {
        let mut cfg = test_config();
        cfg.cache_dir = std::env::temp_dir().join("aitop-panel-cache-test");
        let mut good = Panel::new("copilot");
        good.rows.push(Row::new("premium", 42.0, "166/200".into()));
        write_panel_cache(&cfg.cache_dir, &good);

        let mut failed = Panel::new("copilot");
        failed.error = Some("401 unauthorized".into());
        merge_cached(&cfg, &mut failed);
        assert_eq!(failed.rows[0].pct, 42.0);
        assert!(failed.stale);
        assert_eq!(failed.error.as_deref(), Some("401 unauthorized"));
        let _ = std::fs::remove_dir_all(&cfg.cache_dir);
    }

    #[test]
    fn a_plan_cap_change_is_reported_once() {
        let mut p = Panel::new("z.ai");
        let mut r = Row::new("weekly", 80.0, "4.00M / 5.00M".into());
        r.set_cap("5.00M");
        p.rows.push(r);

        let mut hist = history::History::default();
        assert!(cap_notes(&mut hist, &p, "2026-10-09T00:00:00Z").is_empty());

        // the plan cap changes; the next run says so, then stays quiet
        p.rows[0].set_cap("10.00M");
        assert_eq!(
            cap_notes(&mut hist, &p, "2026-10-10T00:00:00Z"),
            vec!["cap 5.00M → 10.00M (first seen 2026-10-09T00:00:00Z)".to_string()]
        );
        assert!(cap_notes(&mut hist, &p, "2026-10-11T00:00:00Z").is_empty());
    }

    #[test]
    fn local_panels_report_throughput_and_have_no_bars_without_a_cap() {
        let cfg = test_config();
        let stats = local::collect(&[local::Event {
            ts: Utc::now() - chrono::Duration::minutes(10),
            tokens: 600,
            output: 500,
            secs: 10.0,
            cost: 0.0,
            model: "gemma4:26b".into(),
        }]);

        // a provider with no quota API and no configured cap: totals + throughput, no bars
        let p = local_panel(&cfg, "ollama", &stats);
        assert_eq!(p.rows.len(), 4);
        assert!(p.rows.iter().all(|r| r.cap.is_none()));
        assert!(p.rows.iter().all(|r| r.pct == 0.0));
        assert!(p
            .lines
            .iter()
            .any(|l| l == "output tok/s: 50.0 (24h mean) · last 50.0"));
        assert!(p
            .lines
            .iter()
            .any(|l| l.contains("gemma4:26b") && l.contains("· 50 tok/s")));

        // z.ai keeps its bars because ZAI_LIMIT_* is configured
        let z = zai_panel(&cfg, &stats, &[]);
        assert_eq!(z.rows.len(), 4);
        assert!(z.lines.iter().any(|l| l.contains("output tok/s")));
    }
}
