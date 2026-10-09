# AGENTS.md — Developer & AI Agent Guide for `aitop`

`aitop` is a lightweight, single-binary CLI and TUI dashboard ("htop for AI usage") written in Rust. It monitors live quotas, rolling rate limits, token generation throughput, and billing across major LLM providers (**OpenAI Codex**, **Anthropic Claude**, **GitHub Copilot**, **z.ai**, **OpenRouter**, and local session runners).

---

## 1. Architecture & Codebase Map

| File | Purpose | Key Responsibilities |
| :--- | :--- | :--- |
| `src/main.rs` | Entrypoint & Event Loop | CLI argument parsing, crossterm raw mode lifecycle, panic hook (`set_hook`), background refresh thread, `--render-test` harness. |
| `src/ui.rs` | Ratatui Presentation | Drawing routines, responsive 2-column tiling ($\ge 110$ width), focus zoom (`Enter`), detail scrolling (`j`/`k`), fixed-column row formatting (`LABEL_W`, `PCT_W`, `BAR_W`), clean clock header. |
| `src/providers.rs` | Provider Logic | Provider implementations (`codex`, `claude`, `copilot`, `zai`, `openrouter`, `local_panel`), multi-threaded concurrent fetching with `std::thread::scope`, 10s HTTP timeouts, live z.ai quota API parsing, live `x-ratelimit-*` headers, fallback cache. |
| `src/local.rs` | Session Log Analytics | Parser for `~/.pi/agent/sessions/` and Codex rollout logs; AST graph walking via `parentId` to measure true assistant generation tok/s (excluding tool use); 10-day `mtime` file pruning. |
| `src/config.rs` | Configuration | Environment & config loading (`.env`, `~/.config/aitop/.env`, GitHub hosts auth, Codex auth); `Limits` and `budget: Limits`. |
| `src/model.rs` | Domain Models | Core types (`Snapshot`, `Panel`, `Row`), formatting helpers (`fmt_tokens`, `fmt_money`, `fmt_duration`), ASCII bar calculations. |
| `src/pricing.rs` | Cost Estimation | OpenRouter model pricing cache (`~/.cache/aitop/models.json`) with 24h expiration and fallback. |
| `src/history.rs` | Persistence | Cap persistence across runs (`~/.cache/aitop/limits.json`). |
| `src/pace.rs` | Pacing Calculations | Burning pace evaluation against linear elapsed time in rolling windows. |
| `src/util.rs` | System Utilities | Filesystem hygiene (POSIX `0700`/`0600` permissions). |

---

## 2. Core Invariants & Guidelines

Any AI agent modifying this codebase MUST uphold the following rules:

### A. Pure Panel Builders (Testability Invariant)
- **Always decouple networking from panel creation**:
  - Functions performing network I/O (`request()`) fetch raw JSON or headers.
  - Pure builder functions (e.g. `codex_panel`, `claude_panel`, `zai_panel`, `openrouter_panel`) transform structured data into `Panel`.
- **Zero Network Dependencies in Tests**: All unit tests must construct panels from static JSON fixtures and execute in sub-millisecond time. Never write unit tests that hit the internet.

### B. No Heavy Asynchronous Runtimes
- Keep the binary small, simple, and fast.
- Do **NOT** introduce `tokio`, `async-std`, or heavy async libraries.
- Use `ureq` for HTTP requests and `std::thread::scope` for parallel multi-provider fetching.

### C. Bounded Network I/O
- Every HTTP request must respect `const TIMEOUT: std::time::Duration = Duration::from_secs(10)`.
- Never make unbounded network calls that could hang the TUI refresh loop on unstable connections.

### D. Secret Hygiene & Security
- **Never commit `.env` or real API keys**: Active `.env` files must remain in `.gitignore`.
- **Pre-commit Hook**: The repository enforces `.git/hooks/pre-commit` to prevent accidental staging of keys or `.env` files.
- **Key Masking**: Always display API keys using `mask_key()` (first 4 characters followed by ellipsis). Respect `cfg.redact`.
- **File Permissions**: Cache files and directories created on disk must enforce POSIX `0700`/`0600` permissions via `util::ensure_owner_only()`.
- **Test Fixtures**: Only use synthetic dummy keys in tests (e.g., `"3af1deadbeefdeadbeef"`).
- **Git Identity**: Commits must be authored under:
  ```text
  Name:  rafaelzimmermann
  Email: rafaelzimmermann@users.noreply.github.com
  ```

---

## 3. Development & Verification Workflow

Always verify your changes with the following commands before completing any task:

```bash
# 1. Run all unit tests (must pass 100% in 0.00s)
cargo test

# 2. Strict linter check (must produce 0 warnings)
cargo clippy --all-targets -- --deny warnings

# 3. Code formatting
cargo fmt --check

# 4. TUI Render Verification
# Render current live state to stdout without launching full interactive screen:
./target/debug/aitop --render-test

# Test dual-column responsive layout with synthetic panels:
./target/debug/aitop --render-test --synthetic --size 120x30

# Test single-column compact layout with synthetic panels:
./target/debug/aitop --render-test --synthetic --size 100x30

# 5. CLI Plain Snapshot
./target/debug/aitop --plain
```

---

## 4. Provider Integration Conventions

When adding a new provider or updating an existing one:

1. **Provider Resolution**:
   In `src/providers.rs`, update `fetch_provider()`:
   ```rust
   fn fetch_provider(cfg: &Config, pricing: &Pricing, name: &str) -> Panel {
       match name {
           "codex" => codex(cfg, pricing),
           "claude" => claude(cfg, pricing),
           "copilot" => copilot(cfg, pricing),
           "z.ai" | "zai" => zai(cfg, pricing),
           "openrouter" => openrouter(cfg, pricing),
           "new_provider" => new_provider(cfg, pricing),
           other => local_panel(cfg, other, &local::pi_usage(&cfg.pi_session_dir, other, pricing)),
       }
   }
   ```
2. **Multi-Tier Fallback**:
   Always structure providers with graceful degradation:
   - Tier 1: Dedicated Live Quota API (if available).
   - Tier 2: Live HTTP response headers (e.g. `x-ratelimit-*`).
   - Tier 3: Local session logs compared against user-configured `LIMIT_*`.
   - Tier 4: Uncapped local session logs (displaying totals + throughput tok/s without artificial percentage bars).
3. **Throughput Calculation**:
   Always derive token throughput from parent-entry timestamp gaps in `src/local.rs` rather than consecutive assistant messages, ensuring tool execution time is excluded from generation speed calculations.
