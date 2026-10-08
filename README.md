<p align="center">
  <img src="assets/gray-logo.svg" alt="gray" width="96">
</p>
<h1 align="center">gray-web</h1>
<p align="center">Keyless web search — DuckDuckGo and SearXNG with round-robin fallback, no API keys.</p>
<p align="center">
  <a href="https://github.com/vstaln/gray-web/blob/main/LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-7aa2f7.svg">
  <img alt="rust" src="https://img.shields.io/badge/built%20with-rust-orange.svg">
</p>

Keyless web search: `web_query` fans out to DuckDuckGo's html endpoint and
public SearXNG instances — no API keys. API-key engines are covered by gray's
`search` plugin, which is why the tool is named `web_query`, not `web_search`.

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

---
Part of the [gray](https://github.com/vstaln/gray) plugin ecosystem —
the open-source AI agent harness. <https://gray.alignment.id>
