# Code & Architecture Review: `aitop` (Follow-Up Audit)

**Target Crate**: `aitop` (`v0.1.0`)  
**Repository Path**: `/home/spike/workspace/aitop`  
**Review Date**: October 2026 (Updated post-commits `95c0c9c`, `4403239`, `30bf571`)  
**Auditor**: Antigravity (Google DeepMind)  
**Status**: All previous audit findings resolved. Passes 54/54 unit tests in 0.00s. Zero clippy warnings under `--deny warnings`. 100% formatted.

---

## Executive Summary

`aitop` is a lightweight, single-purpose CLI and TUI dashboard ("htop for AI usage") designed to monitor quotas, plan limits, token generation throughput, and billing across major LLM providers (**OpenAI Codex**, **Anthropic Claude**, **GitHub Copilot**, **z.ai**, **OpenRouter**, and generic local runners).

Following our initial code review, three major commits were landed:
1. `95c0c9c` — Implemented bounded HTTP timeouts (10s), concurrent provider fetching via `std::thread::scope`, parent-gap generation throughput calculation (tok/s), OpenRouter periodic budget decoupling, session log mtime pruning, panic hook cleanup, and fixed-column UI alignment.
2. `4403239` — Clamped over-cap percentages to 100% with contextual diagnostic warnings to avoid misleading readings (e.g., 600%) caused by mismatched assumptions.
3. `30bf571` — Discovered and integrated the internal **z.ai Live Quota API** (`GET /api/monitor/usage/quota/limit`), successfully replacing heuristic token accounting with exact upstream server quotas (`CREDIT_LIMIT` / `TOKENS_LIMIT`), true percentage usage, and countdown reset timers.

While backend data collection, concurrency, and reliability are now exemplary (**A+**), the **TUI presentation layer** (`src/ui.rs` and `src/main.rs`) remains ripe for significant ergonomic and visual enhancement.

---

## Updated Scorecard

| Category | Initial Rating | Current Rating | Notes |
| :--- | :---: | :---: | :--- |
| **Architecture & Modularity** | **A** | **A+** | Clean separation of fetch/parse/build; pluggable local provider fallback. |
| **Code Quality & Idiomatic Rust** | **A** | **A+** | Zero unsafe code; zero compiler/clippy warnings under `--deny warnings`. |
| **Test Coverage & Determinism** | **A** | **A+** | Expanded from 49 to 54 deterministic unit tests; 0.00s execution; 0 network dependency. |
| **Secret Handling & Security** | **A** | **A+** | Strict `0700`/`0600` modes; key prefix masking; `--redact` mode; debug-only manifest dir. |
| **Error Resilience & Fallbacks** | **A-** | **A+** | Multi-tier fallback (Live Quota API -> Live headers -> Local logs vs caps -> Uncapped stats). |
| **Concurrency & Networking** | **B+** | **A** | Concurrent fetching with `std::thread::scope`; 10s request timeout on all HTTP calls. |
| **TUI / UX & Presentation** | **B+** | **B+** | Fixed-column layout is clean, but vertical clipping, lack of zoom, and flat text dump limit usability. |

---

## Resolution of Previous Audit Findings

All seven findings raised during the initial audit have been fully resolved:

1. **OpenRouter Pacing Decoupling** (`95c0c9c`): `budget: Limits` added to `Config`; calendar windows pace solely against explicit budgets instead of total lifetime balances.
2. **Bounded HTTP Request Timeouts** (`95c0c9c`): `const TIMEOUT = 10s` applied across all HTTP calls in `providers.rs` and `pricing.rs`.
3. **Concurrent Multi-Provider Fetching** (`95c0c9c`): Replaced sequential fetching with `std::thread::scope`, reducing refresh latency to the duration of the single slowest provider.
4. **Terminal Crash Protection** (`95c0c9c`): `std::panic::set_hook` guarantees `disable_raw_mode()` and alternate screen exit even during unhandled panics.
5. **Claude Credentials Documentation** (`95c0c9c`): Accurately documented that live utilization requires Claude Code OAuth credentials (`~/.claude/.credentials.json`).
6. **Development Manifest Isolation** (`95c0c9c`): Guarded `CARGO_MANIFEST_DIR` fallback with `if cfg!(debug_assertions)` to protect release binaries.
7. **Session Log Discovery Scalability** (`95c0c9c`): Added 10-day `mtime` pruning to `local::walk` to prevent unbounded historical disk scans.

---

## UI / UX Architecture Audit: Current Limitations & Proposed Upgrades

While the backend architecture is now rock-solid, live terminal inspection (`--render-test`) reveals several usability bottlenecks in the interface:

```text
 1 codex  2 z.ai  3 openrouter  · 2026-10-09T13:00:16.675005046+00:00                               
                                                                                                    
┌codex · live quota API + local rollout logs───────────────────────────────────────────────────────┐
│plus · gpt@chess.mozmail.com                                                                      │
│5h window       36%  █████████░░░░░░░░░░░░░░░ resets in 39m · pace ahead 51%                      │
│7d window       24%  ██████░░░░░░░░░░░░░░░░░░ resets in 4d17h · pace on track                     │
│gpt-reserve 7d   0%  ░░░░░░░░░░░░░░░░░░░░░░░░ resets in 7d00h                                     │
│24h tokens        0                                                                               │
│credits: none                                                                                     │
│reset credits available: 3                                                                        │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Limitation 1: Severe Vertical Content Clipping (The $N$-Equal-Slices Problem)
- **The Issue**: In `ui::draw`, vertical space is divided equally among all providers:
  ```rust
  let n = s.snapshot.panels.len().max(1) as u16;
  let per = (chunks[1].height / n).max(3);
  ```
  On a standard 24- to 30-line terminal with 3 providers:
  - Total content height per panel is only **7 to 9 lines** (with 2 rows taken by borders).
  - 1 row is consumed by `subtitle`, 3–4 rows by gauges, and 1 row by the sparkline.
  - Only **1 to 2 lines** remain for `p.lines`!
- **Consequence**: Critical operational data is silently truncated. In Codex, token totals, request counts, and last-request timestamps disappear. In z.ai, estimated cost ($4.98) and the per-model breakdown (`glm-5.3 · 56 tok/s`, `glm-5.2 · 54 tok/s`) are completely hidden. If 4 or 5 providers are enabled, panels collapse into unreadable strips.

### Limitation 2: Wasted Horizontal Space on Modern Displays
- On standard displays ($\ge 100$ columns) or wide monitors (140–200 columns), `LABEL_W` (14) + `PCT_W` (5) + `BAR_W` (24) uses only ~46 columns.
- The remaining 50–150 columns are largely blank space, while the vertical axis suffers from severe data starvation.
- The UI lacks a multi-column or responsive grid layout that can place panels side-by-side on wide screens.

### Limitation 3: Lack of Focus Zoom & Scrollable Viewports
- The top tab bar highlights the active provider (`1 codex`, `2 z.ai`, `3 openrouter`), and the user can cycle focus with `Tab` or `1-9`.
- **However, focus currently does almost nothing**: it merely colors the border cyan.
- There is no **Zoom / Detail mode** (e.g. pressing `Enter` or `z`) to expand the focused provider to full screen to inspect detailed model stats, costs, and token timelines.
- There is no viewport scrolling (`j`/`k`, `PageDown`/`PageUp`); if a panel has 10 lines of detail, lines beyond the second line cannot be viewed.

### Limitation 4: Unstructured Text Dumps vs. Dedicated Ratatui Tables
- `local.rs` collects rich, structured telemetry in `ModelStat`:
  - `model`, `requests`, `tokens`, `output`, `secs`, `cost`, `tps`.
- But `providers.rs` flattens this into unstyled text strings with bullet separators:
  ```text
  glm-5.3                28.27M · 468 req · 56 tok/s
  glm-5.2                14.32M · 346 req · $4.98 · 54 tok/s
  ```
- Because it is rendered via a plain `Paragraph`, there are no table headers, no column alignment for metrics, no color contrast between costs and speeds, and no sorting options.

### Limitation 5: Visual Polish, Typography & Timer Prominence
- **Raw Timestamps**: The header prints `· 2026-10-09T13:00:16.675005046+00:00`. The 9-digit nanosecond tail adds visual noise; formatting as `13:00:16 UTC` would look substantially cleaner.
- **Reset Countdown Prominence**: `resets in 39m` is one of the most vital operational numbers for a developer waiting on rate limits. Currently, it is rendered at the end of the detail string in muted dark gray. It deserves distinct accent styling (e.g., Light Cyan or Magenta with an icon like `⏱ 39m`).
- **Sparkline Timeline Markers**: Sparklines lack time-axis orientation (e.g., `24h ago ──► now`) or peak usage indicators.

---

## Proposed UI Architecture & Upgrades

### Upgrade 1: Responsive Multi-Column Grid Layout
When terminal width allows, automatically tile panels into a 2-column or adaptive grid:
- **Terminal Width < 110 columns**: Single-column vertical stack (current view, but scrollable).
- **Terminal Width $\ge 110$ columns**: Dual-column layout (`Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])`).
- **Impact**: Instantly doubles available vertical height per panel, allowing full model breakdown tables to render without any clipping.

### Upgrade 2: Focused "Zoom" Viewport (`Enter` / `z`)
Implement a two-mode navigation model:
1. **Overview Grid Mode** (default): Shows all providers with gauges and sparklines.
2. **Provider Detail Mode** (`Enter` on focused provider):
   - Expands the selected provider to take 100% of the viewport.
   - Replaces the raw text dump with a rich Ratatui `Table` widget:
     ```text
     ┌ Model ───────────┬── Tokens ───┬── Requests ──┬── Speed ────┬── Est. Cost ──┐
     │ glm-5.3          │      28.27M │          468 │     56 tok/s│         $0.00 │
     │ glm-5.2          │      14.32M │          346 │     54 tok/s│         $4.98 │
     └──────────────────┴─────────────┴──────────────┴─────────────┴───────────────┘
     ```
   - Renders expanded 24-bucket hourly bar charts with peak markers and time labels (`24h ago` -> `now`).
   - Pressing `Esc` or `Enter` toggles back to the multi-provider overview.

### Upgrade 3: Scrollable Panel Viewports (`j` / `k`)
- In `ui::State`, add `scroll_offset: usize`.
- Allow `j`/`k` or `Down`/`Up` to scroll overflowing detail lines in the focused panel.
- Show scrollbar indicators (`▲ 2 more / ▼ 4 more`) when lines exceed available chunk height.

### Upgrade 4: Modernized Header, Status Bar & Modal Help
- **Header**: Format timestamps cleanly: `aitop · 13:00:16 UTC · refresh: 5s`.
- **Status Bar**: Use high-contrast key badges:
  ` [q] Quit  [r] Refresh  [Enter] Zoom  [Tab] Switch  [h] Help `
- **Help Modal**: Render help as a centered floating popup over the screen (`Clear` + `Block` with 60% width / 50% height) rather than crushing the panel layout at the bottom.
- **Key Ergonomics**: Support `?` for help, `vim` keys (`j`/`k`/`h`/`l`), and runtime toggles:
  - `H`: Toggle between 24h hourly and 7d daily sparkline view.
  - `x`: Toggle privacy/redaction mode on the fly.

---

## Detailed Review of New Backend Subsystems

### 1. Live z.ai Quota API Integration (`30bf571`)
- **Discovery**: Reverse-engineered the endpoint used by `glm-plan-usage`:
  ```http
  GET https://api.z.ai/api/monitor/usage/quota/limit
  Authorization: Bearer <ZAI_API_KEY>
  ```
- **Implementation**:
  - `quota_url(cfg)` extracts the host from `ZAI_BASE_URL`, supporting both `api.z.ai` and `open.bigmodel.cn`.
  - `quota_limits()` and `add_quota_rows()` parse `CREDIT_LIMIT` and `TOKENS_LIMIT`:
    - `unit: 3` -> 5h rolling window.
    - `unit: 6` -> weekly window.
    - `TIME_LIMIT` -> MCP / tool call quota.
  - Automatically derives countdown timers from `nextResetTime` timestamps (`resets in 2h51m`, `resets in 2d02h`).
- **Graceful Fallback**: If the quota API fails, `aitop` falls back to inspecting live `x-ratelimit-*` headers from `/models`, and then to local session token accounting against `ZAI_LIMIT_*`.

### 2. True Generation Throughput Telemetry (`local.rs`)
- In `pi` session logs, generation duration cannot be measured by simply taking timestamps between consecutive assistant messages (which would include tool execution time).
- `aitop` indexes parent entries via `parentId`. Generation time is measured strictly between the trigger entry (`role: "user"` or `role: "toolResult"`) and the resulting assistant reply.
- Computes both **24h rolling average tok/s** and **instantaneous last-request tok/s**, rendering per-model throughput metrics (`glm-5.3 · 56 tok/s`, `glm-5.2 · 54 tok/s`).

### 3. Fixed-Column Proportional TUI Layout (`ui.rs`)
- Standardized row rendering using fixed-width spans (`LABEL_W = 14`, `PCT_W = 5`, `BAR_W = 24`), ensuring progress bars, percentages, and reset countdowns align into uniform vertical columns across all providers.

---

## Verification & Test Results

```
running 54 tests
test config::tests::env_helpers_fall_back_to_defaults ... ok
test config::tests::gh_hosts_file_is_parsed ... ok
test config::tests::codex_tokens_come_from_auth_json ... ok
test local::tests::generation_time_comes_from_the_entry_that_triggered_the_call ... ok
test local::tests::chained_assistant_replies_are_not_generation_time ... ok
test local::tests::throughput_is_output_tokens_over_generation_time ... ok
test providers::tests::live_quota_api_rows_replace_the_local_bars ... ok
test providers::tests::quota_api_failure_falls_back_to_local_rows ... ok
test providers::tests::local_panels_report_throughput_and_have_no_bars_without_a_cap ... ok
test ui::tests::rows_use_fixed_columns_so_bars_line_up ... ok
...
test result: ok. 54 passed; 0 failed; 0 ignored; finished in 0.00s
```

- **Clippy**: `cargo clippy --all-targets -- --deny warnings` completed with 0 warnings.
- **Formatting**: `cargo fmt --check` completed cleanly.
- **Live Smoke Test**:
  - `codex`: Live session extraction (36% 5h window, 24% 7d window, reset timers active).
  - `z.ai`: Live quota API operational (1% 5h credit window, 4% weekly credit window, 32.9 mean tok/s).
  - `openrouter`: Paid tier detected, credits balance ($3.58 of $30.00), pricing cache valid (469 models).

---

## Conclusion & Recommended Next Step

`aitop`'s data pipeline, quota discovery, and resilience are top-tier. Upgrading the TUI presentation layer with **responsive 2-column tiling**, a **focused Zoom / Detail mode**, and a **structured model table** will elevate the project from a capable terminal utility into a truly polished developer cockpit.
