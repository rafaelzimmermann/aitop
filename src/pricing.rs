use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::util;

const UA: &str = "aitop/0.1 (+https://github.com/; htop-for-ai-usage)";

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Price {
    pub prompt: f64,
    pub completion: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Pricing {
    pub fetched_at: Option<String>,
    pub source: String,
    pub models: HashMap<String, Price>,
}

impl Pricing {
    /// Look up a model price: exact id, `provider/model`, then a fuzzy name match.
    pub fn lookup(&self, provider: &str, model: &str) -> Option<&Price> {
        if self.models.is_empty() {
            return None;
        }
        let base = model.split(':').next().unwrap_or(model);
        for key in [
            model.to_string(),
            format!("{provider}/{model}"),
            format!("{provider}/{base}"),
            base.to_string(),
        ] {
            if let Some(p) = self.models.get(&key) {
                return Some(p);
            }
        }
        self.models
            .iter()
            .find(|(k, _)| k.ends_with(&format!("/{base}")) || k.as_str() == base)
            .map(|(_, v)| v)
    }

    pub fn cost(
        &self,
        provider: &str,
        model: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> f64 {
        let p = match self.lookup(provider, model) {
            Some(p) => p,
            None => return 0.0,
        };
        input as f64 * p.prompt
            + output as f64 * p.completion
            + cache_read as f64 * p.cache_read
            + cache_write as f64 * p.cache_write
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn is_stale(&self, max_age_hours: i64) -> bool {
        age_hours(self) > max_age_hours as f64
    }
}

fn cache_file(cache_dir: &Path) -> PathBuf {
    cache_dir.join("pricing.json")
}

fn read_cache(cache_dir: &Path) -> Option<Pricing> {
    let raw = std::fs::read_to_string(cache_file(cache_dir)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let mut p = Pricing {
        fetched_at: v
            .get("fetched_at")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        source: "cache".to_string(),
        models: HashMap::new(),
    };
    if let Some(map) = v.get("models").and_then(|m| m.as_object()) {
        for (id, price) in map {
            let num = |k: &str| -> f64 {
                price
                    .get(k)
                    .and_then(|x| {
                        x.as_f64()
                            .or_else(|| x.as_str().and_then(|s| s.parse::<f64>().ok()))
                    })
                    .unwrap_or(0.0)
            };
            let cache_read = num("cache_read");
            let cache_write = num("cache_write");
            p.models.insert(
                id.clone(),
                Price {
                    prompt: num("prompt"),
                    completion: num("completion"),
                    cache_read: if cache_read > 0.0 {
                        cache_read
                    } else {
                        num("input_cache_read")
                    },
                    cache_write: if cache_write > 0.0 {
                        cache_write
                    } else {
                        num("input_cache_write")
                    },
                },
            );
        }
    }
    if p.is_empty() {
        return None;
    }
    Some(p)
}

fn age_hours(p: &Pricing) -> f64 {
    match p
        .fetched_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
    {
        Some(t) => (Utc::now() - t.to_utc()).num_seconds() as f64 / 3600.0,
        None => f64::MAX,
    }
}

/// Fresh cache → live fetch → stale cache. Never fails.
pub fn load(cache_dir: &Path, base: &str, max_age_hours: i64) -> Pricing {
    let cached = read_cache(cache_dir);
    if let Some(c) = &cached {
        if !c.is_stale(max_age_hours) {
            return c.clone();
        }
    }

    let url = format!("{}/models", base.trim_end_matches('/'));
    match ureq::get(&url)
        .timeout(TIMEOUT)
        .set("User-Agent", UA)
        .set("accept", "application/json")
        .call()
    {
        Ok(resp) => {
            let v = match resp.into_json::<serde_json::Value>() {
                Ok(v) => v,
                Err(_) => return cached.unwrap_or_default(),
            };
            let mut p = Pricing {
                fetched_at: Some(Utc::now().to_rfc3339()),
                source: "live".to_string(),
                models: HashMap::new(),
            };
            if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
                for m in arr {
                    let id = match m.get("id").and_then(|i| i.as_str()) {
                        Some(i) => i.to_string(),
                        None => continue,
                    };
                    let price = m.get("pricing");
                    if price.is_none() {
                        continue;
                    }
                    let price = price.unwrap();
                    let num = |k: &str| {
                        price
                            .get(k)
                            .and_then(|x| x.as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(0.0)
                    };
                    p.models.insert(
                        id,
                        Price {
                            prompt: num("prompt"),
                            completion: num("completion"),
                            cache_read: num("input_cache_read"),
                            cache_write: num("input_cache_write"),
                        },
                    );
                }
            }
            if p.is_empty() {
                return cached.unwrap_or_default();
            }
            if let Ok(raw) = serde_json::to_string(&p) {
                util::secret_dir(cache_dir);
                util::write_secret(&cache_file(cache_dir), &raw);
            }
            p
        }
        Err(_) => cached.unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricing() -> Pricing {
        let mut p = Pricing::default();
        p.models.insert(
            "z-ai/glm-5.2".to_string(),
            Price {
                prompt: 0.000000084,
                completion: 0.000008,
                cache_read: 0.000000083,
                cache_write: 0.0,
            },
        );
        p
    }

    #[test]
    fn finds_model_by_provider_prefix() {
        let p = pricing();
        assert!(p.lookup("zai", "glm-5.2").is_some());
        assert!(p.lookup("z-ai", "z-ai/glm-5.2").is_some());
        assert!(p.lookup("zai", "glm-5.2:batch").is_some());
        assert!(p.lookup("zai", "unknown-model").is_none());
    }

    #[test]
    fn cost_uses_per_token_rates() {
        let p = pricing();
        let c = p.cost("zai", "glm-5.2", 100_000, 10_000, 0, 0);
        assert!((c - (100_000.0 * 0.000000084 + 10_000.0 * 0.000008)).abs() < 1e-9);
    }

    #[test]
    fn staleness_is_measured_against_the_configured_max_age() {
        let mut p = Pricing::default();
        assert!(p.is_stale(24), "never fetched → stale");
        p.fetched_at = Some(Utc::now().to_rfc3339());
        assert!(!p.is_stale(24));
        p.fetched_at = Some((Utc::now() - chrono::Duration::hours(25)).to_rfc3339());
        assert!(p.is_stale(24));
    }

    #[test]
    fn cache_roundtrip() {
        let dir = std::env::temp_dir().join("aitop-test-cache");
        let _ = std::fs::create_dir_all(&dir);
        let mut p = pricing();
        p.fetched_at = Some(Utc::now().to_rfc3339());
        let raw = serde_json::to_string(&p).unwrap();
        std::fs::write(cache_file(&dir), raw).unwrap();
        let back = read_cache(&dir).unwrap();
        assert_eq!(back.source, "cache");
        assert_eq!(back.len(), 1);
        assert!(back.lookup("zai", "glm-5.2").is_some());
        let orig = p.models["z-ai/glm-5.2"];
        let back_price = back.models["z-ai/glm-5.2"];
        assert!(back_price.prompt > 0.0);
        assert_eq!(back_price, orig);
        let before = p.cost("zai", "glm-5.2", 100_000, 10_000, 5_000, 1_000);
        let after = back.cost("zai", "glm-5.2", 100_000, 10_000, 5_000, 1_000);
        assert!((before - after).abs() < 1e-12);
        assert!(before > 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
