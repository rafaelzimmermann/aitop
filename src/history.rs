use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Local};

use crate::util;

const FILE: &str = "limits.json";

/// Usage inside a rolling window only ever goes up, so a pct drop of this
/// size between two sightings means the window rolled over.
const ROLLOVER_DROP: f64 = 50.0;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Entry {
    cap: String,
    first_seen: String,
    last_seen: String,
    prev: Option<String>,
    #[serde(default)]
    last_pct: Option<f64>,
}

/// Per-provider/window caps seen across runs, so a plan change shows up as
/// "cap 5.00M → 10.00M" instead of a hardcoded ZAI_LIMIT_*.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct History {
    seen: BTreeMap<String, Entry>,
}

impl History {
    pub fn load(dir: &Path) -> History {
        std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|raw| serde_json::from_str::<History>(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) {
        if self.seen.is_empty() {
            return;
        }
        if let Ok(raw) = serde_json::to_string_pretty(self) {
            util::secret_dir(dir);
            util::write_secret(&dir.join(FILE), &raw);
        }
    }

    /// Record the cap currently in use for provider/window. Returns a note when
    /// the cap differs from the last one we saw (the first sighting is silent).
    pub fn note(&mut self, provider: &str, window: &str, cap: &str, now: &str) -> Option<String> {
        if cap.is_empty() {
            return None;
        }
        let key = format!("{provider}/{window}");
        let entry = self.seen.entry(key).or_insert_with(|| Entry {
            cap: cap.to_string(),
            first_seen: now.to_string(),
            last_seen: now.to_string(),
            prev: None,
            last_pct: None,
        });
        if !entry.cap.is_empty() && entry.cap == cap {
            entry.last_seen = now.to_string();
            return None;
        }
        if entry.cap.is_empty() {
            // the entry was created by window() tracking alone; adopting a cap
            // is not a plan change, so stay silent
            entry.cap = cap.to_string();
            entry.last_seen = now.to_string();
            return None;
        }
        let prev = entry.cap.clone();
        entry.prev = Some(prev.clone());
        entry.cap = cap.to_string();
        entry.last_seen = now.to_string();
        Some(format!(
            "cap {prev} → {cap} (first seen {})",
            entry.first_seen
        ))
    }

    /// Track a live quota window across refreshes. A fresh window reading ~0%
    /// right after a provider said "usage limit reached" looks like a bug, so
    /// spell the rollover out: `5h window just reset (was 100% at 13:36)`.
    pub fn window(&mut self, provider: &str, window: &str, pct: f64, now: &str) -> Option<String> {
        let key = format!("{provider}/{window}");
        let entry = self.seen.entry(key).or_insert_with(|| Entry {
            cap: String::new(),
            first_seen: now.to_string(),
            last_seen: now.to_string(),
            prev: None,
            last_pct: None,
        });
        let prev_pct = entry.last_pct.replace(pct);
        let prev_seen = entry.last_seen.clone();
        entry.last_seen = now.to_string();
        match prev_pct {
            Some(prev) if prev - pct >= ROLLOVER_DROP => Some(format!(
                "{window} just reset (was {prev:.0}% at {})",
                clock(&prev_seen)
            )),
            _ => None,
        }
    }
}

/// HH:MM in the local timezone, for notes the user compares against the reset
/// timestamps their provider CLI prints.
fn clock(rfc3339: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_is_silent_but_recorded() {
        let mut h = History::default();
        assert_eq!(
            h.note("z.ai", "weekly", "5.00M", "2026-10-09T00:00:00Z"),
            None
        );
        assert_eq!(
            h.note("z.ai", "weekly", "5.00M", "2026-10-10T00:00:00Z"),
            None
        );
    }

    #[test]
    fn a_changed_cap_is_reported_once_per_run() {
        let mut h = History::default();
        h.note("z.ai", "weekly", "5.00M", "2026-10-09T00:00:00Z");
        assert_eq!(
            h.note("z.ai", "weekly", "10.00M", "2026-10-10T00:00:00Z"),
            Some("cap 5.00M → 10.00M (first seen 2026-10-09T00:00:00Z)".into())
        );
        assert_eq!(
            h.note("z.ai", "weekly", "10.00M", "2026-10-11T00:00:00Z"),
            None
        );
        // other providers and windows are tracked independently
        assert_eq!(
            h.note("codex", "7d window", "10.00M", "2026-10-11T00:00:00Z"),
            None
        );
    }

    #[test]
    fn an_empty_cap_is_not_recorded() {
        let mut h = History::default();
        assert_eq!(
            h.note("openrouter", "credits", "", "2026-10-09T00:00:00Z"),
            None
        );
        assert!(h.seen.is_empty());
    }

    #[test]
    fn a_rollover_is_reported_once_then_goes_quiet() {
        let mut h = History::default();
        // observe the window filling up
        assert_eq!(
            h.window("z.ai", "5h window", 40.0, "2026-10-10T11:00:00+00:00"),
            None
        );
        assert_eq!(
            h.window("z.ai", "5h window", 100.0, "2026-10-10T11:35:00+00:00"),
            None
        );
        // the window rolls: the CLI just said "usage limit reached" yet the
        // panel reads 1% — say why instead of looking broken
        let note = h.window("z.ai", "5h window", 1.0, "2026-10-10T11:37:00+00:00");
        assert_eq!(
            note.as_deref(),
            Some(
                format!(
                    "5h window just reset (was 100% at {})",
                    clock("2026-10-10T11:35:00+00:00")
                )
                .as_str()
            )
        );
        // and only once
        assert_eq!(
            h.window("z.ai", "5h window", 3.0, "2026-10-10T11:38:00+00:00"),
            None
        );
    }

    #[test]
    fn small_pct_dips_are_not_rollovers() {
        let mut h = History::default();
        h.window("z.ai", "weekly", 60.0, "2026-10-10T11:00:00+00:00");
        assert_eq!(
            h.window("z.ai", "weekly", 20.0, "2026-10-10T11:30:00+00:00"),
            None
        );
        // a drop just under the threshold stays silent too
        h.window("z.ai", "weekly", 100.0, "2026-10-10T12:00:00+00:00");
        assert_eq!(
            h.window("z.ai", "weekly", 51.0, "2026-10-10T12:30:00+00:00"),
            None
        );
    }

    #[test]
    fn window_tracking_coexists_with_cap_notes() {
        let mut h = History::default();
        h.window("z.ai", "5h window", 100.0, "2026-10-10T11:35:00+00:00");
        // adopting a cap into an entry created by window() is silent
        assert_eq!(
            h.note("z.ai", "5h window", "12.0k", "2026-10-10T11:36:00+00:00"),
            None
        );
        assert_eq!(
            h.note("z.ai", "5h window", "12.0k", "2026-10-10T11:37:00+00:00"),
            None
        );
        let dir = std::env::temp_dir().join("aitop-test-history-pct");
        let _ = std::fs::create_dir_all(&dir);
        h.save(&dir);
        let back = History::load(&dir);
        assert_eq!(
            back.seen.get("z.ai/5h window").and_then(|e| e.last_pct),
            Some(100.0)
        );
        // the rollover still fires after a restart
        assert_eq!(
            {
                let mut h2 = History::load(&dir);
                h2.window("z.ai", "5h window", 1.0, "2026-10-10T11:40:00+00:00")
            }
            .as_deref(),
            Some(
                format!(
                    "5h window just reset (was 100% at {})",
                    clock("2026-10-10T11:37:00+00:00")
                )
                .as_str()
            )
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_roundtrip_survives_and_corruption_is_tolerated() {
        let dir = std::env::temp_dir().join("aitop-test-history");
        let _ = std::fs::create_dir_all(&dir);
        let mut h = History::default();
        h.note("z.ai", "weekly", "5.00M", "2026-10-09T00:00:00Z");
        h.save(&dir);
        assert_eq!(
            History::load(&dir)
                .seen
                .get("z.ai/weekly")
                .map(|e| e.cap.as_str()),
            Some("5.00M")
        );

        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert!(History::load(&dir).seen.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
