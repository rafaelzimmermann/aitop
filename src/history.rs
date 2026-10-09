use std::collections::BTreeMap;
use std::path::Path;

use crate::util;

const FILE: &str = "limits.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Entry {
    cap: String,
    first_seen: String,
    last_seen: String,
    prev: Option<String>,
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
        });
        if entry.cap == cap {
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
