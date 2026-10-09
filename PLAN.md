# aitop — next steps

State at this commit: crate compiles, `cargo fmt --check` and
`cargo clippy --all-targets -- --deny warnings` are clean, `cargo test` reports
35 passing tests (config, local, model, pace, pricing, providers, ui, util, main).
Now 41 with the history and `--history` tests.
Smoke-tested against live endpoints with `--plain`, `--plain --redact` and `--json`.

## 1. Test pass — done

- [x] `local.rs` cost assertion (`2.1e-4` → `2.0e-4`)
- [x] `config.rs` — `parse_github_token(raw)` / `parse_auth_token(raw, key)` + `test_config()`
- [x] `model.rs` — duration/window/token/money formatting, `Row::new` leaves `pace = None`
- [x] `main.rs` — `ascii_bar` clamping, `sparkline()`, `--watch N` parsing, pricing reuse
- [x] `ui.rs` — `bar_color` thresholds, `panel_title` truncation
- [x] `providers.rs` — pure panel builders + tests for codex, claude, copilot, openrouter,
      z.ai local rows, unknown provider, redaction, last-good fallback
- [x] tests are network-free and deterministic (no `Utc::now()`-dependent assertions
      except explicit elapsed-time setups; `Pricing::default()` is used instead of the cache)

## 2. Provider refactor — done

Each provider is `fetch → parse → panel_builder`; the JSON→Panel part is pure and tested.

## 3. Displayed features — done

- [x] `Row.pace` rendered in the TUI gauge label and `--plain` (`· pace ahead 30%`)
- [x] `Panel.stale` set on failed fetches (last-good panel) and rendered dimmed
- [x] `Panel.source` in the panel header
- [x] pricing reloaded when the cache goes stale (`PRICING_CACHE_HOURS`), in the TUI
      thread and in `--plain/--json --watch`

## 4. Secret hygiene — done

- [x] `.env` gitignored, `.env.example` has no values
- [x] cached codex token written `0600`
- [x] key prefix shortened to 4 chars (`mask_key`)
- [x] `~/.cache/aitop` created `0700` (`util::secret_dir`), files `0600` (`util::write_secret`)
- [x] codex email kept in `--json` (already in local credential files), documented in README
- [x] `--redact` flag / `AITOP_REDACT` strips email + key prefixes

## 5. Docs / packaging — done

- [x] README: claude + copilot rows, pace section, pricing cache, new env vars, `--watch`, `--redact`
- [x] `.env.example` matches the names the code actually reads
- [x] `cargo build --release --locked` is clean

## 6. Remaining

- [x] persist per-provider limits history so pace can compare against plan caps
      (`src/history.rs`, `~/.cache/aitop/limits.json`: `{provider/window: {cap, first_seen,
      last_seen, prev}}`), so a plan change is visible as "cap went 5M → 10M" instead of a
      hardcoded `ZAI_LIMIT_*`. Rows carry a `cap` field (also in `--json`); the first sighting
      is silent, a change prints one line per run.
- [x] z.ai: derive the cap from the live `x-ratelimit-*` headers when the gateway sends
      them, falling back to `ZAI_LIMIT_*` (`live_limits` in providers.rs; checked 2026-10-09:
      `api.z.ai` returns no `x-ratelimit-*` headers, so the probe only takes effect if they appear).
- [x] optional: `--history` sparkline over 7d instead of 24h (`Stats.daily`, 7 daily buckets,
      exposed as `spark_label` in plain output and `--json`)
- [x] TUI rows use fixed columns (label 14 · pct 5 · bar 24 · detail) so bars line up across
      rows regardless of label length; `Gauge` replaced by `Paragraph` spans, empty bar cells
      dimmed, one palette (`Cyan` accent / `DarkGray` muted / `Gray` text) with green/yellow/red
      thresholds and pace colors
- [x] throughput: output tok/s from the parent→assistant gap in pi session logs
      (`Event.secs`, `Stats.tps_24h`/`last_tps`, `ModelStat.tps`), and any provider name without
      a quota API (`PROVIDERS=ollama,strata`) falls back to that local-log panel
- [x] CI runs `cargo fmt --check`; the tree is formatted, keep it that way
