use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::pricing::Pricing;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelStat {
    pub model: String,
    pub requests: u64,
    pub tokens: u64,
    pub output: u64,
    pub secs: f64,
    pub cost: f64,
    pub tps: Option<f64>,
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
    pub daily: Vec<u64>, // last 7 daily buckets, oldest first
    pub cost_total: f64,
    pub cost_24h: f64,
    /// output tokens generated inside the 24h window
    pub output_24h: u64,
    /// generation time measured inside the 24h window
    pub secs_24h: f64,
    pub tps_24h: Option<f64>,
    pub last_tps: Option<f64>,
    pub models: Vec<ModelStat>,
    pub first_5h: Option<DateTime<Utc>>,
    pub first_24h: Option<DateTime<Utc>>,
    pub first_7d: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub ts: DateTime<Utc>,
    pub tokens: u64,
    pub output: u64,
    /// generation time in seconds; 0.0 when the log has no usable parent entry
    pub secs: f64,
    pub cost: f64,
    pub model: String,
}

pub fn collect(events: &[Event]) -> Stats {
    let now = Utc::now();
    let mut s = Stats::default();
    let mut buckets = vec![0u64; 24];
    let mut daily = vec![0u64; 7];
    let mut newest: Option<DateTime<Utc>> = None;
    let mut newest_tps: Option<(DateTime<Utc>, f64)> = None;
    let mut out24 = 0u64;
    let mut secs24 = 0.0f64;
    let mut models: BTreeMap<String, ModelStat> = BTreeMap::new();
    // output tokens that carry a measured generation time, per model
    let mut timed_output: BTreeMap<String, u64> = BTreeMap::new();

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
            if e.output > 0 && e.secs > 0.0 {
                out24 += e.output;
                secs24 += e.secs;
            }
            s.first_24h = Some(s.first_24h.map(|f| e.ts.min(f)).unwrap_or(e.ts));
        }
        if e.ts > now - Duration::days(7) {
            s.tokens_7d += e.tokens;
            let age_days = ((now - e.ts).num_days().max(0)) as usize;
            if age_days < 7 {
                daily[6 - age_days] += e.tokens;
            }
            s.first_7d = Some(s.first_7d.map(|f| e.ts.min(f)).unwrap_or(e.ts));
        }
        if e.ts > now - Duration::hours(24) {
            // clock skew (a timestamp in the future) clamps to the current hour
            let age_hours = ((now - e.ts).num_seconds().max(0) / 3600) as usize;
            if age_hours < 24 {
                buckets[23 - age_hours] += e.tokens;
            }
        }
        if newest.map(|n| e.ts > n).unwrap_or(true) {
            newest = Some(e.ts);
        }

        if e.output > 0 && e.secs > 0.0 {
            let tps = e.output as f64 / e.secs;
            if newest_tps.map(|(t, _)| e.ts > t).unwrap_or(true) {
                newest_tps = Some((e.ts, tps));
            }
        }

        let key = if e.model.is_empty() {
            "unknown".to_string()
        } else {
            e.model.clone()
        };
        let m = models.entry(key.clone()).or_default();
        if m.model.is_empty() {
            m.model = key;
        }
        m.requests += 1;
        m.tokens += e.tokens;
        m.output += e.output;
        m.secs += e.secs;
        m.cost += e.cost;
        if e.output > 0 && e.secs > 0.0 {
            let cur = timed_output.get(&m.model).copied().unwrap_or(0) + e.output;
            timed_output.insert(m.model.clone(), cur);
        }
    }

    s.output_24h = out24;
    s.secs_24h = secs24;
    s.tps_24h = if secs24 > 0.0 {
        Some(out24 as f64 / secs24)
    } else {
        None
    };
    s.last_tps = newest_tps.map(|(_, v)| v);
    for m in models.values_mut() {
        let timed = timed_output.get(&m.model).copied().unwrap_or(0);
        m.tps = if m.secs > 0.0 {
            Some(timed as f64 / m.secs)
        } else {
            None
        };
    }

    s.last_request = newest.map(|t| t.to_rfc3339());
    s.spark = buckets;
    s.daily = daily;
    s.models = models.into_values().collect();
    s.models.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    s
}

/// Collect .jsonl session files modified within `max_age` (the largest window we
/// account over), so a growing archive of old sessions is never re-read.
fn walk(dir: &Path, out: &mut Vec<PathBuf>, max_age: std::time::Duration) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out, max_age);
            } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                let fresh = match e.metadata().and_then(|m| m.modified()) {
                    Ok(t) => match SystemTime::now().duration_since(t) {
                        Ok(age) => age <= max_age,
                        Err(_) => true, // mtime in the future: keep it
                    },
                    Err(_) => true, // no mtime available: keep it
                };
                if fresh {
                    out.push(p);
                }
            }
        }
    }
}

/// Largest window `collect` buckets over is 7 days; keep a margin for clock skew.
fn walk_all(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(dir, &mut files, std::time::Duration::from_secs(10 * 86400));
    files.sort();
    files
}

fn ts(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    v.get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.to_utc())
}

#[derive(Debug)]
struct Entry {
    role: String,
    ts: DateTime<Utc>,
}

/// id -> entry, so an assistant message can be timed against the entry that triggered it.
fn index(raw: &str) -> BTreeMap<String, Entry> {
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        let d: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if d.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let t = match ts(&d) {
            Some(t) => t,
            None => continue,
        };
        let id = match d.get("id").and_then(|i| i.as_str()) {
            Some(i) => i.to_string(),
            None => continue,
        };
        out.insert(
            id,
            Entry {
                role: d
                    .get("message")
                    .and_then(|m| m.get("role"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string(),
                ts: t,
            },
        );
    }
    out
}

/// Generation time: the gap between the entry that triggered the call (a user message or
/// a tool result) and the assistant reply. Assistant parents are chained replies, so the
/// gap there includes tool execution and is not generation time.
fn generation_secs(index: &BTreeMap<String, Entry>, d: &serde_json::Value) -> f64 {
    let own = match ts(d) {
        Some(t) => t,
        None => return 0.0,
    };
    let parent = match d
        .get("parentId")
        .and_then(|p| p.as_str())
        .and_then(|p| index.get(p))
    {
        Some(e) if e.role == "user" || e.role == "toolResult" => e,
        _ => return 0.0,
    };
    let secs = (own - parent.ts).num_nanoseconds().unwrap_or(0) as f64 / 1e9;
    if secs > 0.0 {
        secs
    } else {
        0.0
    }
}

/// pi session logs: assistant messages carry per-request usage with provider tag.
pub fn pi_events(raw: &str, provider: &str, pricing: &Pricing) -> Vec<Event> {
    let entries = index(raw);
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
        let (input, output, cr, cw) = (
            num("input"),
            num("output"),
            num("cacheRead"),
            num("cacheWrite"),
        );
        let tokens = usage
            .get("totalTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(input + output);
        if tokens == 0 {
            continue;
        }
        let model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let logged = usage
            .get("cost")
            .and_then(|c| c.get("total"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let cost = if logged > 0.0 {
            logged
        } else {
            pricing.cost(provider, &model, input, output, cr, cw)
        };
        if let Some(t) = ts(&d) {
            out.push(Event {
                ts: t,
                tokens,
                output,
                secs: generation_secs(&entries, &d),
                cost,
                model,
            });
        }
    }
    out
}

pub fn pi_usage(dir: &Path, provider: &str, pricing: &Pricing) -> Stats {
    let files = walk_all(dir);
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
        let info = payload
            .and_then(|p| p.get("info"))
            .or_else(|| d.get("info"));
        let last = match info.and_then(|i| i.get("last_token_usage")) {
            Some(l) => l,
            None => continue,
        };
        let num = |k: &str| last.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let tokens = last
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if tokens == 0 {
            continue;
        }
        let model = info
            .and_then(|i| i.get("model"))
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let cost = pricing.cost(
            "openai",
            &model,
            num("input_tokens"),
            num("output_tokens"),
            num("cached_input_tokens"),
            num("cache_write_input_tokens"),
        );
        if let Some(t) = ts(&d) {
            out.push(Event {
                ts: t,
                tokens,
                output: num("output_tokens"),
                // rollout logs record usage, not duration, so throughput is not measurable here
                secs: 0.0,
                cost,
                model,
            });
        }
    }
    out
}

pub fn codex_usage(dir: &Path, pricing: &Pricing) -> Stats {
    let files = walk_all(dir);
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
            crate::pricing::Price {
                prompt: 1e-7,
                completion: 1e-6,
                cache_read: 0.0,
                cache_write: 0.0,
            },
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
    fn generation_time_comes_from_the_entry_that_triggered_the_call() {
        let raw = r#"{"type":"message","id":"a","timestamp":"2026-10-09T05:00:00Z","message":{"role":"toolResult","content":[]}}
{"type":"message","id":"b","parentId":"a","timestamp":"2026-10-09T05:00:10Z","message":{"role":"assistant","provider":"ollama","model":"gemma4:26b","usage":{"input":100,"output":500,"totalTokens":600}}}
"#;
        let e = pi_events(raw, "ollama", &Pricing::default());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].output, 500);
        assert_eq!(e[0].secs, 10.0);
    }

    #[test]
    fn chained_assistant_replies_are_not_generation_time() {
        let raw = r#"{"type":"message","id":"a","timestamp":"2026-10-09T05:00:00Z","message":{"role":"assistant","provider":"ollama","usage":{"output":10}}}
{"type":"message","id":"b","parentId":"a","timestamp":"2026-10-09T05:00:10Z","message":{"role":"assistant","provider":"ollama","usage":{"output":500}}}
"#;
        let e = pi_events(raw, "ollama", &Pricing::default());
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].secs, 0.0);
    }

    #[test]
    fn throughput_is_output_tokens_over_generation_time() {
        let now = Utc::now();
        let events = vec![
            Event {
                ts: now - Duration::minutes(10),
                tokens: 600,
                output: 500,
                secs: 10.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::hours(2),
                tokens: 300,
                output: 200,
                secs: 20.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::days(3),
                tokens: 400,
                output: 300,
                secs: 30.0,
                cost: 0.0,
                model: "m".into(),
            },
        ];
        let s = collect(&events);
        assert_eq!(s.output_24h, 700); // the 3-day-old event is outside the window
        assert_eq!(s.secs_24h, 30.0);
        assert!((s.tps_24h.unwrap() - 23.333333333333332).abs() < 1e-9);
        assert_eq!(s.last_tps, Some(50.0)); // most recent event: 500 output / 10s
        assert!((s.models[0].tps.unwrap() - 16.666666666666668).abs() < 1e-9);
    }

    #[test]
    fn no_throughput_without_measured_generation_time() {
        let raw = r#"{"type":"message","timestamp":"2026-10-09T05:00:00Z","message":{"role":"assistant","provider":"zai","usage":{"output":500}}}"#;
        let s = collect(&pi_events(raw, "zai", &Pricing::default()));
        assert_eq!(s.tps_24h, None);
        assert_eq!(s.last_tps, None);
        assert_eq!(s.models[0].tps, None);
    }

    #[test]
    fn collect_buckets_and_windows() {
        let now = Utc::now();
        let events = vec![
            Event {
                ts: now - Duration::minutes(30),
                tokens: 100,
                output: 0,
                secs: 0.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::hours(10),
                tokens: 200,
                output: 0,
                secs: 0.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::days(3),
                tokens: 400,
                output: 0,
                secs: 0.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::days(30),
                tokens: 800,
                output: 0,
                secs: 0.0,
                cost: 0.0,
                model: "m".into(),
            },
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
        assert_eq!(s.daily.len(), 7);
        assert_eq!(s.daily.iter().sum::<u64>(), 700);
        assert_eq!(s.daily[6], 300); // today: 30min ago + 10h ago
        assert_eq!(s.daily[3], 400); // three days ago
        assert_eq!(s.daily[0], 0); // the 30-day-old event is outside the window
        assert_eq!(
            s.last_request.as_deref().unwrap(),
            events[0].ts.to_rfc3339().as_str()
        );
        assert_eq!(s.models[0].requests, 4);
    }

    #[test]
    fn throughput_uses_output_tokens_over_generation_time() {
        let now = Utc::now();
        let events = vec![
            Event {
                ts: now - Duration::hours(2),
                tokens: 1000,
                output: 600,
                secs: 20.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::hours(3),
                tokens: 500,
                output: 400,
                secs: 10.0,
                cost: 0.0,
                model: "m".into(),
            },
            // outside the 24h window: counted for the model, not for the 24h mean
            Event {
                ts: now - Duration::days(30),
                tokens: 900,
                output: 900,
                secs: 9.0,
                cost: 0.0,
                model: "m".into(),
            },
        ];
        let s = collect(&events);
        assert_eq!(s.output_24h, 1000);
        assert_eq!(s.secs_24h, 30.0);
        assert_eq!(s.tps_24h, Some(1000.0 / 30.0));
        assert_eq!(s.last_tps, Some(30.0)); // newest event: 600 / 20
        assert_eq!(s.models[0].tps, Some(1900.0 / 39.0));
    }

    #[test]
    fn future_events_do_not_panic_or_corrupt_buckets() {
        let now = Utc::now();
        let events = vec![Event {
            ts: now + Duration::hours(2),
            tokens: 123,
            output: 0,
            secs: 0.0,
            cost: 0.0,
            model: "m".into(),
        }];
        let s = collect(&events);
        assert_eq!(s.total, 123);
        assert_eq!(s.spark.iter().sum::<u64>(), 123);
        assert_eq!(s.spark[23], 123); // clamped to the current hour
        assert_eq!(s.daily.iter().sum::<u64>(), 123);
        assert_eq!(s.daily[6], 123);
    }

    #[test]
    fn hourly_buckets_place_current_hour_at_end_and_preserve_slot_zero() {
        let now = Utc::now();
        let events = vec![
            Event {
                ts: now - Duration::minutes(30), // age_hours == 0 → bucket 23
                tokens: 10,
                output: 0,
                secs: 0.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::hours(1), // age_hours == 1 → bucket 22
                tokens: 20,
                output: 0,
                secs: 0.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::hours(23), // age_hours == 23 → bucket 0
                tokens: 30,
                output: 0,
                secs: 0.0,
                cost: 0.0,
                model: "m".into(),
            },
        ];
        let s = collect(&events);
        assert_eq!(s.spark.len(), 24);
        assert_eq!(s.spark[23], 10);
        assert_eq!(s.spark[22], 20);
        assert_eq!(s.spark[0], 30);
        assert_eq!(s.spark.iter().sum::<u64>(), 60);
    }

    #[test]
    fn model_throughput_excludes_untimed_output() {
        let now = Utc::now();
        let events = vec![
            Event {
                ts: now - Duration::minutes(1),
                tokens: 100,
                output: 100,
                secs: 10.0,
                cost: 0.0,
                model: "m".into(),
            },
            Event {
                ts: now - Duration::minutes(2),
                tokens: 900,
                output: 900,
                secs: 0.0, // no measured generation time: excluded from tok/s
                cost: 0.0,
                model: "m".into(),
            },
        ];
        let s = collect(&events);
        assert_eq!(s.models[0].tps, Some(10.0)); // 100 timed output / 10s
        assert_eq!(s.tps_24h, Some(10.0));
    }
}
