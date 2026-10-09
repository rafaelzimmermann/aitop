# aitop — next steps

State at `97f6d20`: crate compiles, 11 tests pass (pace 4, pricing 3, local 4).
Uncommitted work in progress: `local.rs` cost assertion fixed (expected `2.0e-4`),
`config.rs` test helper + tests partially added (edit failed, needs re-apply).

## 1. Finish the test pass (in progress)

- [x] `local.rs` — cost estimate assertion was wrong (`2.1e-4` → `2.0e-4`)
- [ ] `config.rs` — split `github_token` into `parse_github_token(raw)` and
      `codex_access_token`/`codex_refresh_token` into `parse_auth_token(raw, key)`
      so they are testable without env/files; add `#[cfg(test)] test_config()`
      (tests already drafted, the edit did not apply — re-run it against the
      current file text)
- [ ] `model.rs` — `fmt_duration` (`45s`, `12m`, `3h16m`, `5d01h`), `window_label`
      (`5h window`, `7d window`, `1w window`), `fmt_tokens` (`999`, `1.5k`, `2.00M`),
      `fmt_money`, `Row::new` leaves `pace = None`
- [ ] `main.rs` — `ascii_bar` (0%, 50%, 100%, clamping above 100%) and the plain
      renderer's sparkline block
- [ ] `ui.rs` — `bar_color` thresholds (green <70, yellow 70–89, red ≥90)
- [ ] `providers.rs` — needs pure panel builders first (see §2), then tests for
      codex `rate_limit` + `additional_rate_limits` + credits, claude tier/expiry +
      utilization windows, copilot `percent_remaining` vs `remaining/entitlement`,
      openrouter key/credits + daily/weekly/monthly pace, z.ai local rows
- [ ] keep tests network-free and deterministic (no `Utc::now()`-dependent
      assertions except where the elapsed time is set explicitly)

## 2. Make the providers testable

Refactor each `pub fn xxx()` into fetch + pure panel builder:

```rust
pub fn codex(cfg, pricing) -> Panel            // already pure: codex_panel(cfg, pricing, &v)
pub fn claude(cfg, pricing) -> Panel           // → claude_panel(cfg, pricing, oauth, usage)
pub fn copilot(cfg, pricing) -> Panel          // → copilot_panel(cfg, &v)
pub fn openrouter(cfg, pricing) -> Panel       // → openrouter_panel(cfg, pricing, data, credits)
pub fn zai(cfg, pricing) -> Panel              // → zai_panel(cfg, stats, probe_lines)
```

Behaviour must stay identical; only the JSON→Panel part becomes a pure function.

## 3. Wire up the features that exist but are not displayed

- `Row.pace` is computed but never rendered → show it in the TUI gauge label and in
  `--plain` output (e.g. `· pace ahead 30%`)
- `Panel.stale` is never set → mark panels built from the pricing cache / stale token
  cache, and render them dimmed
- `Panel.source` is set but only visible in `--json` → show it in the panel header
- pricing is loaded once at startup; the refresh thread should reload it when the
  cache goes stale (`PRICING_CACHE_HOURS`) instead of keeping the startup snapshot

## 4. Secret hygiene (from the security review)

- [x] `.env` is gitignored and absent from the commit; `.env.example` has no values
- [x] cached codex token is written `0600`
- [ ] shorten the z.ai key prefix in the panel subtitle from 8 to 4 chars
      (`providers.rs:416`) — `--json` output is often piped to files/logs
- [ ] create `~/.cache/aitop` as `0700` (it is `0755` today, so the token file lives
      in a world-readable directory even though the file itself is `0600`)
- [ ] decide whether the codex account email belongs in `--json` (PII in scripted
      output); if kept, document it in the README
- [ ] add a `--redact` flag that strips key prefixes/emails from `--json`

## 5. Docs / packaging

- README still lists only codex, z.ai, openrouter — add claude and copilot rows
- document the new env vars: `PROVIDERS`, `PACE_TRIGGER`, `PRICING_CACHE_HOURS`,
  `AITOP_CACHE_DIR`, `CLAUDE_CREDENTIALS_FILE`, `ANTHROPIC_BASE_URL`,
  `GITHUB_TOKEN`/`GH_TOKEN`, `GITHUB_API_BASE_URL`
- `.env.example` is missing those same vars
- README should state the pace feature and the pricing cache
- `install.sh`: `--locked` fails without a lockfile matching the manifest — verify
  `cargo build --release --locked` is clean before relying on it

## 6. Nice to have

- `cargo clippy --all-targets` clean (one dead-code warning: `Config.home`)
- CI: build + test + clippy on push
- `--plain --watch N` for non-TUI scripting
- persist per-provider limits history so pace can compare against plan caps
