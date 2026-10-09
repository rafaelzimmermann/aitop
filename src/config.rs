use std::path::PathBuf;

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

    pub home: PathBuf,
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

fn env_float(key: &str, default: f64) -> f64 {
    env_opt(key)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

/// GitHub token: env wins, then gh's own config file.
fn github_token(h: &PathBuf) -> Option<String> {
    for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Some(t) = env_opt(k) {
            return Some(t);
        }
    }
    let raw = std::fs::read_to_string(h.join(".config/gh/hosts.yml")).ok()?;
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
        .map(|s| s.split(',').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect())
        .unwrap_or_else(|| vec!["codex".into(), "z.ai".into(), "openrouter".into()]);

    Config {
        refresh_secs: env_num("REFRESH_SECONDS", 5),
        home: h.clone(),
        cache_dir: PathBuf::from(env("AITOP_CACHE_DIR", &h.join(".cache/aitop").display().to_string())),

        codex_auth_file: PathBuf::from(env("CODEX_AUTH_FILE", &h.join(".codex/auth.json").display().to_string())),
        codex_token: env_opt("CODEX_ACCESS_TOKEN"),
        codex_base: env("CODEX_BASE_URL", "https://chatgpt.com/backend-api"),
        codex_installation_id: env_opt("CODEX_INSTALLATION_ID")
            .or_else(|| std::fs::read_to_string(h.join(".codex/installation_id")).ok().map(|s| s.trim().to_string())),
        codex_client_id: env("CODEX_CLIENT_ID", "app_EMoamEEZ73f0CkXaXp7hrann"),
        auth_base: env("OPENAI_AUTH_BASE_URL", "https://auth.openai.com"),

        claude_credentials_file: PathBuf::from(env("CLAUDE_CREDENTIALS_FILE", &h.join(".claude/.credentials.json").display().to_string())),
        anthropic_base: env("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),

        github_token: github_token(&h),
        github_base: env("GITHUB_API_BASE_URL", "https://api.github.com"),

        zai_key: env_opt("ZAI_API_KEY"),
        zai_base: env("ZAI_BASE_URL", "https://api.z.ai/api/coding/paas/v4"),

        openrouter_key: env_opt("OPENROUTER_API_KEY"),
        openrouter_base: env("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1"),

        codex_session_dir: PathBuf::from(env("CODEX_SESSION_DIR", &h.join(".codex/sessions").display().to_string())),
        pi_session_dir: PathBuf::from(env("PI_SESSION_DIR", &h.join(".pi/agent/sessions").display().to_string())),

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

/// Read the OAuth access token from codex's auth.json (preferred) or .env.
pub fn codex_access_token(cfg: &Config) -> Option<String> {
    if let Some(t) = &cfg.codex_token {
        return Some(t.clone());
    }
    let raw = std::fs::read_to_string(&cfg.codex_auth_file).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("tokens")
        .and_then(|t| t.get("access_token"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
}

pub fn codex_refresh_token(cfg: &Config) -> Option<String> {
    let raw = std::fs::read_to_string(&cfg.codex_auth_file).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("tokens")
        .and_then(|t| t.get("refresh_token"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
}
