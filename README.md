# aitop

`htop` for AI usage — a live terminal dashboard for provider quotas.

```
 1 codex  2 claude  3 copilot  4 z.ai  5 openrouter  · 2026-10-09T05:23:31Z
┌codex · live quota API────────────────────────────────┐
│plus · you@example.com                                │
│███              5h window   3%  resets in 3h16m      │
│████████████████ 7d window  17%  resets in 5d01h      │
│                          gpt-reserve 7d  0%  ...     │
└──────────────────────────────────────────────────────┘
┌z.ai · local accounting (not a server quota)──────────┐
│key 3af1… · no public quota API → local accounting    │
│██████████████████ 5h window  80%  159.0k / 200.0k    │
│                       24h tokens  1.2M  ▁▂▄▆█▇▆▃     │
└──────────────────────────────────────────────────────┘
```

Panel titles show the data source, and `stale` when the numbers come from the last good
snapshot instead of a live fetch.

## Run

```bash
./install.sh            # build + install to ~/.local/bin (seeds ~/.config/aitop/.env)
./install.sh --run      # install and print one snapshot
./install.sh --prefix /usr/local
./install.sh --uninstall
```

Then:

```bash
aitop                  # TUI
aitop --plain          # one-shot text snapshot (scriptable)
aitop --plain --watch 30
aitop --json           # one-shot JSON snapshot
aitop --json --redact  # no email / key prefix in the output
aitop --plain --history # sparkline over 7 daily buckets instead of 24 hourly ones
aitop --help
```

TUI keys: `q` quit · `r` refresh now · `h` help · `1-9` focus provider · `Tab`/`↑`/`↓` cycle.

## Data sources

| provider   | source                                                            |
|------------|-------------------------------------------------------------------|
| codex      | `GET https://chatgpt.com/backend-api/codex/usage` (OAuth token from `~/.codex/auth.json`) + local `~/.codex/sessions/**/rollout-*.jsonl` (`token_count` events) |
| claude     | `GET https://api.anthropic.com/api/oauth/usage` (OAuth refresh token from `~/.claude/.credentials.json`, or an `sk-` key) + local pi logs |
| copilot    | `GET https://api.github.com/copilot_internal/user` (`GITHUB_TOKEN`, `gh` token, or `~/.config/gh/oauth_token`) |
| z.ai       | **no public quota API** → local accounting from `~/.pi/agent/sessions` (pi assistant messages with `usage`, provider `zai`) compared against configurable `ZAI_LIMIT_*`; plus a live `/models` probe that prints any `x-ratelimit-*` headers the gateway returns |
| openrouter | `GET https://openrouter.ai/api/v1/key` and `/api/v1/credits`     |
| any other  | no quota API → local session logs (`PROVIDERS=ollama,strata`), reported as totals plus output tok/s (generation time measured from each assistant message to its parent) |

Model pricing for the local accounting comes from `GET {OPENROUTER_BASE_URL}/models`,
cached in `~/.cache/aitop/pricing.json` for `PRICING_CACHE_HOURS` (24 by default).

## Pace

Every window with a known start prints its pace next to the bar: usage is compared with
the share of the window already elapsed, and labelled `pace ahead / on track / behind`
using `PACE_TRIGGER` (10 by default). A window that started 50% ago at 20% usage is
`pace ahead 30%` — you are burning the quota slower than the clock.

Windows are only known when the provider reports them (`resets_at` / `resets_at_seconds`)
or when local logs can infer the first request in the window.

## Config

Lookup order: `AITOP_ENV` → `./.env` → `~/.config/aitop/.env` → project `.env`.
`install.sh` seeds `~/.config/aitop/.env` (chmod 600) from the project `.env` if it exists, otherwise from `.env.example`.

| var | meaning |
|-----|---------|
| `REFRESH_SECONDS` | TUI refresh interval (default 5) |
| `CODEX_AUTH_FILE` | path to `~/.codex/auth.json` (or set `CODEX_ACCESS_TOKEN`) |
| `CODEX_BASE_URL` | default `https://chatgpt.com/backend-api` |
| `CODEX_INSTALLATION_ID` | sent as `x-codex-installation-id`; auto-read from `~/.codex/installation_id` |
| `CLAUDE_CREDENTIALS_FILE` | path to `~/.claude/.credentials.json` (OAuth creds; `sk-` keys are not accepted by the usage endpoint — without them the row falls back to local accounting) |
| `ANTHROPIC_BASE_URL` | default `https://api.anthropic.com` |
| `GITHUB_TOKEN` | copilot token; falls back to `gh`'s stored token |
| `GITHUB_API_BASE_URL` | default `https://api.github.com` |
| `ZAI_API_KEY` / `ZAI_BASE_URL` | z.ai key + `https://api.z.ai/api/coding/paas/v4` |
| `ZAI_LIMIT_5H` / `ZAI_LIMIT_DAY` / `ZAI_LIMIT_WEEK` / `ZAI_LIMIT_RPM` | assumed caps for the local z.ai accounting — tune to your plan; usage above a cap is clamped to 100% and flagged as "over assumed cap", it is a wrong guess, not an exhausted quota |
| `OPENROUTER_API_KEY` / `OPENROUTER_BASE_URL` | OpenRouter key + base |
| `OR_BUDGET_DAY` / `OR_BUDGET_WEEK` / `OR_BUDGET_MONTH` | optional daily/weekly/monthly spending budgets (USD); pace on OpenRouter calendar rows is computed against these, not against your lifetime balance |
| `CODEX_SESSION_DIR` / `PI_SESSION_DIR` | local session-log directories used for the 24h sparkline and local accounting |
| `AITOP_CACHE_DIR` | `~/.cache/aitop` — panel snapshots, pricing cache and `limits.json`, created 0700, files 0600 |
| `PRICING_CACHE_HOURS` | pricing cache TTL (default 24) |
| `PACE_TRIGGER` | pace ahead/behind threshold (default 10) |
| `AITOP_REDACT` | `1` hides email + key prefixes (same as `--redact`) |
| `PROVIDERS` | comma-separated panel list and order (default `codex,z.ai,openrouter`) |

The Codex usage endpoint returns `403` without a `codex_cli_rs/*` User-Agent, so the client sends one.

## Notes

- z.ai bars are **estimates**: they are local token accounting against limits you configure, not server-reported quotas.
- Local accounting only counts what is written to the session logs; anything done through other tools/clients is invisible.
- `tok/s` is output tokens divided by the parent→assistant timestamp gap in the session logs.
  It measures generation only: prefill time is not separable from the logs, so the number is an
  upper bound on end-to-end throughput.
- Secrets are never printed — only a masked key prefix.
- `--json` includes the account email when a provider reports one (it is already in your
  local credential files and in the TUI panel header). Pipe it to a file or use `--redact`
  if you do not want identity in the output.
- `--json` is a single snapshot unless you pass `--watch N`; nothing is written to stdout
  by the refresh thread itself.
- Caps in use are remembered per provider/window in `~/.cache/aitop/limits.json`. When a cap
  changes between runs (you tune `ZAI_LIMIT_*`, or a plan changes), the panel prints
  `cap 5.00M → 10.00M (first seen …)` once. `--json` rows carry `cap` for the same reason.
