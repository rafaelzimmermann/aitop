# aitop

`htop` for AI usage — a live terminal dashboard for provider quotas.

```
 1 codex  2 z.ai  3 openrouter  · 2026-10-09T05:23:31Z
┌codex────────────────────────────────────────────────┐
│plus · you@example.com                                │
│███              5h window   3%  resets in 3h16m      │
│████████████████ 7d window  17%  resets in 5d01h      │
│                          gpt-reserve 7d  0%  ...     │
└──────────────────────────────────────────────────────┘
┌z.ai─────────────────────────────────────────────────┐
│key 3af1bb28… · no public quota API → local accounting│
│██████████████████ 5h window  80%  159.0k / 200.0k    │
...
```

## Run

```bash
./install.sh            # build + install to ~/.local/bin (seeds ~/.config/aitop/.env)
./install.sh --run      # install and print one snapshot
./install.sh --prefix /usr/local
./install.sh --uninstall
```

Then:

```bash
aitop            # TUI
aitop --plain    # one-shot text snapshot (scriptable)
aitop --json     # one-shot JSON snapshot
aitop --help
```

TUI keys: `q` quit · `r` refresh now · `h` help · `1-9` focus provider · `Tab`/`↑`/`↓` cycle.

## Data sources

| provider   | source                                                            |
|------------|-------------------------------------------------------------------|
| codex      | `GET https://chatgpt.com/backend-api/codex/usage` (OAuth token from `~/.codex/auth.json`) + local `~/.codex/sessions/**/rollout-*.jsonl` (`token_count` events) |
| z.ai       | **no public quota API** → local accounting from `~/.pi/agent/sessions` (pi assistant messages with `usage`, provider `zai`) compared against configurable `ZAI_LIMIT_*`; plus a live `/models` probe that prints any `x-ratelimit-*` headers the gateway returns |
| openrouter | `GET https://openrouter.ai/api/v1/key` and `/api/v1/credits`     |

## Config

Lookup order: `AITOP_ENV` → `./.env` → `~/.config/aitop/.env` → project `.env`.
`install.sh` seeds `~/.config/aitop/.env` (chmod 600) from the project `.env` if it exists, otherwise from `.env.example`.

| var | meaning |
|-----|---------|
| `REFRESH_SECONDS` | TUI refresh interval (default 5) |
| `CODEX_AUTH_FILE` | path to `~/.codex/auth.json` (or set `CODEX_ACCESS_TOKEN`) |
| `CODEX_BASE_URL` | default `https://chatgpt.com/backend-api` |
| `CODEX_INSTALLATION_ID` | sent as `x-codex-installation-id`; auto-read from `~/.codex/installation_id` |
| `ZAI_API_KEY` / `ZAI_BASE_URL` | z.ai key + `https://api.z.ai/api/coding/paas/v4` |
| `ZAI_LIMIT_5H` / `ZAI_LIMIT_DAY` / `ZAI_LIMIT_WEEK` / `ZAI_LIMIT_RPM` | assumed caps for the local z.ai accounting — tune to your plan |
| `OPENROUTER_API_KEY` / `OPENROUTER_BASE_URL` | OpenRouter key + base |
| `CODEX_SESSION_DIR` / `PI_SESSION_DIR` | local session-log directories used for the 24h sparkline and z.ai accounting |

The Codex usage endpoint returns `403` without a `codex_cli_rs/*` User-Agent, so the client sends one.

## Notes

- z.ai bars are **estimates**: they are local token accounting against limits you configure, not server-reported quotas.
- Local accounting only counts what is written to the session logs; anything done through other tools/clients is invisible.
- Secrets are never printed — only a masked key prefix.
