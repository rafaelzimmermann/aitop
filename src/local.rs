use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use chrono::{DateTime, Duration, Utc};

use crate::pricing::Pricing;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelStat {
    pub model: String,
    pub requests: u64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Stats {
    pub tokens_5h: u64,
    pub tokens_24h: u64,
    pub tokens_7d: u64,
    pub total: u64,
    pub requests: u64,
    pub requests_5h: u64,
    pub last_request: Option<String>,
    pub spark: Vec<u64>, // last 24 hourly buckets, oldest first
    pub cost_total: f64,
    pub cost_24h: f64,
    pub models: Vec<ModelStat>,
    pub first_5h: Option<DateTime<Utc>>,
    pub first_24h: Option<DateTime<Utc>>,
    pub first_7d: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub ts: DateTime<Utc>,
    pub tokens: u64,
    pub cost: f64,
    pub model: String,
}

fn collect(events: &[Event]) -> Stats {
    let now = Utc::now();
    let mut s = Stats::default();
    let mut buckets = vec![0u64; 24];
    let mut newest: Option<DateTime<Utc>> = None;
    let mut models: BTreeMap<String, ModelStat> = BTreeMap::new();

    for e in events {
        s.total += e.tokens;
        s.cost_total += e.cost;
        s.requests += 1;

        if e.ts > now - Duration::hours(5) {
            s.tokens_5h += e.tokens;
            s.requests_5h += 1;
            s.first_5h = Some(s.first_5h.map(|f| e.ts.min(f)).unwrap_or(e.ts));
        }
        if e.ts > now - Duration::hours(24) {
            s.tokens_24h += e.tokens;
            s.cost_24h += e.cost;
            s.first_24h = Some(s.first_24h.map(|f| e.ts.min(f)).unwrap_or(e.ts));
        }
        if e.ts > now - Duration::days(7) {
            s.tokens_7d += e.tokens;
            s.first_7d = Some(s.first_7d.map(|f| e.ts.min(f)).unwrap_or(e.ts));
        }
        if e.ts > now - Duration::hours(24) {
            let idx = (24 - (((now - e.ts).num_seconds() / 3600) as usize)).min(23);
            buckets[idx] += e.tokens;
        }
        if newest.map(|n| e.ts > n).unwrap_or(true) {
            newest = Some(e.ts);
        }

        let key = if e.model.is_empty() { "unknown".to_string() } else { e.model.clone() };
        let m = models.entry(key).or_default();
        m.requests += 1;
        m.tokens += e.tokens;
        m.cost += e.cost;
    }

    s.last_request = newest.map(|t| t.to_rfc3339());
    s.spark = buckets;
    s.models = models.into_values().collect();
    s.models.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    s
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
}

fn ts(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    v.get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.to_utc())
}

/// pi session logs: assistant messages carry per-request usage with provider tag.
pub fn pi_events(raw: &str, provider: &str, pricing: &Pricing) -> Vec<Event> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let d: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let msg = match d.get("message") {
            Some(m) => m,
            None => continue,
        };
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        if msg.get("provider").and_then(|p| p.as_str()) != Some(provider) {
            continue;
        }
        let usage = match msg.get("usage") {
            Some(u) => u,
            None => continue,
        };
        let num = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let (input, output, cr, cw) = (num("input"), num("output"), num("cacheRead"), num("cacheWrite"));
        let tokens = usage.get("totalTokens").and_then(|v| v.as_u64()).unwrap_or(input + output);
        if tokens == 0 {
            continue;
        }
        let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let logged = usage.get("cost").and_then(|c| c.get("total")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cost = if logged > 0.0 {
            logged
        } else {
            pricing.cost(provider, &model, input, output, cr, cw)
        };
        if let Some(t) = ts(&d) {
            out.push(Event { ts: t, tokens, cost, model });
        }
    }
    out
}

pub fn pi_usage(dir: &Path, provider: &str, pricing: &Pricing) -> Stats {
    let mut files = Vec::new();
    walk(dir, &mut files);
    let mut events = Vec::new();
    for f in files {
        if let Ok(raw) = std::fs::read_to_string(&f) {
            events.extend(pi_events(&raw, provider, pricing));
        }
    }
    collect(&events)
}

/// codex rollout logs: token_count entries with last_token_usage.
pub fn codex_events(raw: &str, pricing: &Pricing) -> Vec<Event> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let d: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let payload = d.get("payload");
        let ptype = payload
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
            .or_else(|| d.get("type").and_then(|t| t.as_str()));
        if ptype != Some("token_count") {
            continue;
        }
        let info = payload.and_then(|p| p.get("info")).or_else(|| d.get("info"));
        let last = match info.and_then(|i| i.get("last_token_usage")) {
            Some(l) => l,
            None => continue,
        };
        let num = |k: &str| last.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let tokens = last.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        if tokens == 0 {
            continue;
        }
        let model = info.and_then(|i| i.get("model")).and_then(|m| m.as_str()).unwrap_or("").to_string();
        let cost = pricing.cost(
            "openai",
            &model,
            num("input_tokens"),
            num("output_tokens"),
            num("cached_input_tokens"),
            num("cache_write_input_tokens"),
        );
        if let Some(t) = ts(&d) {
            out.push(Event { ts: t, tokens, cost, model });
        }
    }
    out
}

pub fn codex_usage(dir: &Path, pricing: &Pricing) -> Stats {
    let mut files = Vec::new();
    walk(dir, &mut files);
    let mut events = Vec::new();
    for f in files {
        if let Ok(raw) = std::fs::read_to_string(&f) {
            events.extend(codex_events(&raw, pricing));
        }
    }
    collect(&events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_events_keep_only_the_requested_provider() {
        let raw = r#"{"type":"message","timestamp":"2026-10-09T05:00:00Z","message":{"role":"assistant","provider":"zai","model":"glm-5.2","usage":{"input":100,"output":50,"totalTokens":150}}}
{"type":"message","timestamp":"2026-10-09T05:01:00Z","message":{"role":"assistant","provider":"ollama","model":"x","usage":{"totalTokens":999}}}
{"type":"model_change","timestamp":"2026-10-09T05:02:00Z"}
"#;
        let e = pi_events(raw, "zai", &Pricing::default());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].tokens, 150);
        assert_eq!(e[0].model, "glm-5.2");
    }

    #[test]
    fn pi_events_estimate_cost_when_the_log_has_none() {
        let raw = r#"{"type":"message","timestamp":"2026-10-09T05:00:00Z","message":{"role":"assistant","provider":"zai","model":"glm-5.2","usage":{"input":1000,"output":100,"totalTokens":1100}}}"#;
        let mut p = Pricing::default();
        p.models.insert(
            "z-ai/glm-5.2".to_string(),
            crate::pricing::Price { prompt: 1e-7, completion: 1e-6, cache_read: 0.0, cache_write: 0.0 },
        );
        let e = pi_events(raw, "zai", &p);
        assert_eq!(e.len(), 1);
        assert!((e[0].cost - 2.0e-4).abs() < 1e-9);
    }

    #[test]
    fn codex_events_read_usage_from_the_payload() {
        let raw = r#"{"timestamp":"2026-10-09T05:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-6-astra","last_token_usage":{"input_tokens":10,"output_tokens":5,"cached_input_tokens":0,"cache_write_input_tokens":0,"total_tokens":15}}}}
{"timestamp":"2026-10-09T05:00:01Z","type":"event_msg","payload":{"type":"other"}}
"#;
        let e = codex_events(raw, &Pricing::default());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].tokens, 15);
        assert_eq!(e[0].model, "gpt-6-astra");
    }

    #[test]
    fn collect_buckets_and_windows() {
        let now = Utc::now();
        let events = vec![
            Event { ts: now - Duration::minutes(30), tokens: 100, cost: 0.0, model: "m".into() },
            Event { ts: now - Duration::hours(10), tokens: 200, cost: 0.0, model: "m".into() },
            Event { ts: now - Duration::days(3), tokens: 400, cost: 0.0, model: "m".into() },
            Event { ts: now - Duration::days(30), tokens: 800, cost: 0.0, model: "m".into() },
        ];
        let s = collect(&events);
        assert_eq!(s.requests, 4);
        assert_eq!(s.total, 1500);
        assert_eq!(s.tokens_5h, 100);
        assert_eq!(s.requests_5h, 1);
        assert_eq!(s.tokens_24h, 300);
        assert_eq!(s.tokens_7d, 700);
        assert_eq!(s.spark.len(), 24);
        assert_eq!(s.spark.iter().sum::<u64>(), 300);
        assert_eq!(s.last_request.as_deref().unwrap(), events[0].ts.to_rfc3339().as_str());
        assert_eq!(s.models[0].requests, 4);
    }
}
