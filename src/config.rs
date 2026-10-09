use std::path::{Path, PathBuf};

/// Rolling-window token limits (z.ai exposes no public quota API, so these are
/// user-configurable and used purely for local accounting bars).
#[derive(Clone, Debug)]
pub struct Limits {
    pub five_hour: u64,
    pub day: u64,
    pub week: u64,
    pub rpm: u64,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub refresh_secs: u64,
    /// hide account identity (email, key prefixes) in output
    pub redact: bool,
    /// sparkline over 7 daily buckets instead of 24 hourly ones
    pub history: bool,

    pub cache_dir: PathBuf,

    pub codex_auth_file: PathBuf,
    pub codex_token: Option<String>,
    pub codex_base: String,
    pub codex_installation_id: Option<String>,
    pub codex_client_id: String,
    pub auth_base: String,

    pub claude_credentials_file: PathBuf,
    pub anthropic_base: String,

    pub github_token: Option<String>,
    pub github_base: String,

    pub zai_key: Option<String>,
    pub zai_base: String,

    pub openrouter_key: Option<String>,
    pub openrouter_base: String,

    pub codex_session_dir: PathBuf,
    pub pi_session_dir: PathBuf,

    pub zai_limits: Limits,

    pub pace_trigger: f64,
    pub pricing_max_age_hours: i64,
    pub providers: Vec<String>,
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_num(key: &str, default: u64) -> u64 {
    env_opt(key)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    match env_opt(key) {
        Some(v) => matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

fn env_float(key: &str, default: f64) -> f64 {
    env_opt(key)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

fn parse_github_token(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with("oauth_token:") {
            let t = line.split_once(':')?.1.trim().trim_matches('"');
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// GitHub token: env wins, then gh's own config file.
fn github_token(h: &Path) -> Option<String> {
    for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Some(t) = env_opt(k) {
            return Some(t);
        }
    }
    std::fs::read_to_string(h.join(".config/gh/hosts.yml"))
        .ok()
        .and_then(|raw| parse_github_token(&raw))
}

pub fn load() -> Config {
    // .env lookup: AITOP_ENV > ./.env > ~/.config/aitop/.env > build dir
    if let Some(p) = env_opt("AITOP_ENV") {
        let _ = dotenvy::from_filename(&p);
    } else if dotenvy::dotenv().is_err() {
        let xdg = home().join(".config/aitop/.env");
        if xdg.exists() {
            let _ = dotenvy::from_filename(&xdg);
        } else {
            let _ = dotenvy::from_filename(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".env"));
        }
    }

    let h = home();
    let providers = env_opt("PROVIDERS")
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_lowercase())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_else(|| vec!["codex".into(), "z.ai".into(), "openrouter".into()]);

    Config {
        refresh_secs: env_num("REFRESH_SECONDS", 5),
        redact: env_bool("AITOP_REDACT", false),
        history: env_bool("AITOP_HISTORY", false),
        cache_dir: PathBuf::from(env(
            "AITOP_CACHE_DIR",
            &h.join(".cache/aitop").display().to_string(),
        )),

        codex_auth_file: PathBuf::from(env(
            "CODEX_AUTH_FILE",
            &h.join(".codex/auth.json").display().to_string(),
        )),
        codex_token: env_opt("CODEX_ACCESS_TOKEN"),
        codex_base: env("CODEX_BASE_URL", "https://chatgpt.com/backend-api"),
        codex_installation_id: env_opt("CODEX_INSTALLATION_ID").or_else(|| {
            std::fs::read_to_string(h.join(".codex/installation_id"))
                .ok()
                .map(|s| s.trim().to_string())
        }),
        codex_client_id: env("CODEX_CLIENT_ID", "app_EMoamEEZ73f0CkXaXp7hrann"),
        auth_base: env("OPENAI_AUTH_BASE_URL", "https://auth.openai.com"),

        claude_credentials_file: PathBuf::from(env(
            "CLAUDE_CREDENTIALS_FILE",
            &h.join(".claude/.credentials.json").display().to_string(),
        )),
        anthropic_base: env("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),

        github_token: github_token(&h),
        github_base: env("GITHUB_API_BASE_URL", "https://api.github.com"),

        zai_key: env_opt("ZAI_API_KEY"),
        zai_base: env("ZAI_BASE_URL", "https://api.z.ai/api/coding/paas/v4"),

        openrouter_key: env_opt("OPENROUTER_API_KEY"),
        openrouter_base: env("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1"),

        codex_session_dir: PathBuf::from(env(
            "CODEX_SESSION_DIR",
            &h.join(".codex/sessions").display().to_string(),
        )),
        pi_session_dir: PathBuf::from(env(
            "PI_SESSION_DIR",
            &h.join(".pi/agent/sessions").display().to_string(),
        )),

        zai_limits: Limits {
            five_hour: env_num("ZAI_LIMIT_5H", 200_000),
            day: env_num("ZAI_LIMIT_DAY", 1_000_000),
            week: env_num("ZAI_LIMIT_WEEK", 5_000_000),
            rpm: env_num("ZAI_LIMIT_RPM", 30),
        },

        pace_trigger: env_float("PACE_TRIGGER", 10.0),
        pricing_max_age_hours: env_num("PRICING_CACHE_HOURS", 24) as i64,
        providers,
    }
}

fn parse_auth_token(raw: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get("tokens")
        .and_then(|t| t.get(key))
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn codex_access_token(cfg: &Config) -> Option<String> {
    if let Some(t) = &cfg.codex_token {
        return Some(t.clone());
    }
    parse_auth_token(
        &std::fs::read_to_string(&cfg.codex_auth_file).ok()?,
        "access_token",
    )
}

pub fn codex_refresh_token(cfg: &Config) -> Option<String> {
    parse_auth_token(
        &std::fs::read_to_string(&cfg.codex_auth_file).ok()?,
        "refresh_token",
    )
}

#[cfg(test)]
pub fn test_config() -> Config {
    Config {
        refresh_secs: 5,
        redact: false,
        history: false,
        cache_dir: PathBuf::from("/tmp/aitop-test-cache"),
        codex_auth_file: PathBuf::from("/tmp/aitop-test-home/.codex/auth.json"),
        codex_token: None,
        codex_base: "https://chatgpt.com/backend-api".into(),
        codex_installation_id: None,
        codex_client_id: "test-client".into(),
        auth_base: "https://auth.openai.com".into(),
        claude_credentials_file: PathBuf::from("/tmp/aitop-test-home/.claude/.credentials.json"),
        anthropic_base: "https://api.anthropic.com".into(),
        github_token: None,
        github_base: "https://api.github.com".into(),
        zai_key: None,
        zai_base: "https://api.z.ai/api/coding/paas/v4".into(),
        openrouter_key: None,
        openrouter_base: "https://openrouter.ai/api/v1".into(),
        codex_session_dir: PathBuf::from("/tmp/aitop-test-home/.codex/sessions"),
        pi_session_dir: PathBuf::from("/tmp/aitop-test-home/.pi/agent/sessions"),
        zai_limits: Limits {
            five_hour: 200_000,
            day: 1_000_000,
            week: 5_000_000,
            rpm: 30,
        },
        pace_trigger: 10.0,
        pricing_max_age_hours: 24,
        providers: vec!["codex".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_helpers_fall_back_to_defaults() {
        std::env::remove_var("AITOP_TEST_NUM");
        std::env::remove_var("AITOP_TEST_FLOAT");
        std::env::remove_var("AITOP_TEST_OPT");
        assert_eq!(env_num("AITOP_TEST_NUM", 5), 5);
        assert_eq!(env_float("AITOP_TEST_FLOAT", 10.0), 10.0);
        assert_eq!(env_opt("AITOP_TEST_OPT"), None);

        std::env::set_var("AITOP_TEST_NUM", " 12 ");
        std::env::set_var("AITOP_TEST_FLOAT", "7.5");
        std::env::set_var("AITOP_TEST_OPT", "   ");
        assert_eq!(env_num("AITOP_TEST_NUM", 5), 12);
        assert_eq!(env_float("AITOP_TEST_FLOAT", 10.0), 7.5);
        assert_eq!(env_opt("AITOP_TEST_OPT"), None, "blank env is not a value");
    }

    #[test]
    fn gh_hosts_file_is_parsed() {
        let raw = "github.com:\n  oauth_token: ght_abc123\n  user: me\n";
        assert_eq!(parse_github_token(raw).as_deref(), Some("ght_abc123"));
        assert_eq!(
            parse_github_token("github.com:\n  oauth_token: \"ght_quoted\"\n").as_deref(),
            Some("ght_quoted")
        );
        assert_eq!(parse_github_token("github.com:\n  oauth_token: \n"), None);
    }

    #[test]
    fn codex_tokens_come_from_auth_json() {
        let dir = std::env::temp_dir().join("aitop-test-auth");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("auth.json");
        std::fs::write(
            &path,
            r#"{"tokens":{"access_token":"tok-a","refresh_token":"tok-r"}}"#,
        )
        .unwrap();

        let mut cfg = test_config();
        cfg.codex_auth_file = path.clone();
        assert_eq!(codex_access_token(&cfg).as_deref(), Some("tok-a"));
        assert_eq!(codex_refresh_token(&cfg).as_deref(), Some("tok-r"));

        // an explicit env token wins over the file
        cfg.codex_token = Some("from-env".into());
        assert_eq!(codex_access_token(&cfg).as_deref(), Some("from-env"));
        assert_eq!(codex_refresh_token(&cfg).as_deref(), Some("tok-r"));

        cfg.codex_auth_file = dir.join("missing.json");
        cfg.codex_token = None;
        assert_eq!(codex_access_token(&cfg), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
