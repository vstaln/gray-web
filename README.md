# gray-web

Keyless web search: `web_query` fans out to DuckDuckGo's html endpoint and
public SearXNG instances — no API keys. Port of the keyless half of
hermes's `web/` providers (`ddgs`, `searxng`); API-key engines are covered
by gray's `search` plugin, which is why the tool is named `web_query`, not
`web_search`.

A sidecar plugin for [gray](https://github.com/vstaln/gray), scaffolded by
[gray-account](https://github.com/vstaln/gray-account).

## Tool

`web_query` — `{query, backend?, limit?}`

- `backend:"auto"` (default) — DuckDuckGo first, then SearXNG instances
  round-robin on failure. `backend:"ddg"` / `"searxng"` pins a backend.
- `limit` — max results (default 8, clamped 1–25).
- Output: `1. <title> — <url>\n<snippet>` lines.

## SearXNG instances

Read from `~/.gray/web/instances.txt` (one per line, `#` comments); three
defaults are baked in. `SEARXNG_URL` is prepended when set — point it at
your own instance, as most public instances bot-wall `format=json`.
A cursor in `~/.gray/web/rr` round-robins the starting instance across
calls.

## Wire methods

`plugin/manifest`, `plugin/shutdown`, `tool/call`. No hooks, no commands,
no host capabilities. All network I/O shells out to curl.

## Install

```sh
gray plugin install web
```

## Develop

```sh
cargo test
gray account check      # entry point + manifest handshake
gray account publish    # check → build → release → publish to the gray registry
```

Bump `version` in `Cargo.toml` before each `publish`; the registry refuses to
republish a version.
