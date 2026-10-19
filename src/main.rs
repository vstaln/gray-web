//! gray-web — keyless web search for the agent.
//!
//! Port of the keyless half of hermes's `web/` providers (ddgs + searxng —
//! API-key engines are covered by gray's `search` plugin): a `web_query`
//! tool that hits DuckDuckGo's html endpoint and a list of public SearXNG
//! instances (`~/.gray/web/instances.txt`, three baked in), auto-falling
//! back round-robin on failure.
//!
//! Named `web_query`, not `web_search` — graysearch owns that name.
//! All network goes through curl; errors surface as friendly tool errors.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

const MAX_BODY: usize = 4 * 1024 * 1024;
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const DEFAULT_LIMIT: usize = 8;
const DEFAULT_INSTANCES: &[&str] = &[
    "https://searx.be",
    "https://baresearch.org",
    "https://search.inetol.net",
];

fn manifest() -> Value {
    json!({
        "name": "web",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [{
            "name": "web_query",
            "description": "Search the web without API keys. backend 'auto' (default) tries DuckDuckGo then public SearXNG instances round-robin; 'ddg' or 'searxng' pins a backend. Returns numbered 'title — url' lines with snippets.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query." },
                    "backend": { "type": "string", "enum": ["auto", "ddg", "searxng"],
                                 "description": "auto (default) | ddg | searxng." },
                    "limit": { "type": "integer", "description": "Max results, default 8." }
                },
                "required": ["query"]
            }
        }],
        "commands": [],
    })
}

fn state_dir() -> std::path::PathBuf {
    let home = std::env::var_os("GRAY_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| std::path::PathBuf::from(h).join(".gray"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join("web")
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// `curl -D` wants a file for the dumped headers; `/dev/stderr` is
/// unix-only, so each call dumps to a scratch file in the temp dir and
/// stderr stays free for `curl:` error lines.
fn header_dump() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "gray-web-{}-{}.hdr",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

fn get(url: &str) -> Result<(Option<u16>, String), String> {
    let dump = header_dump();
    let out = std::process::Command::new("curl")
        .args([
            "-sS",
            "-L",
            "--max-time",
            "20",
            "--compressed",
            "--max-filesize",
            &MAX_BODY.to_string(),
            "-D",
        ])
        .arg(&dump)
        .args(["-A", UA, "--", url])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    let headers = std::fs::read_to_string(&dump).unwrap_or_default();
    let _ = std::fs::remove_file(&dump);
    let status = headers
        .lines()
        .filter(|l| l.starts_with("HTTP/"))
        .last()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse::<u16>().ok());
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr);
        let detail = detail
            .lines()
            .filter(|l| l.starts_with("curl:"))
            .last()
            .map(str::trim)
            .unwrap_or("curl failed");
        return Err(detail.to_string());
    }
    Ok((status, String::from_utf8_lossy(&out.stdout).to_string()))
}

#[derive(Default, Debug, PartialEq)]
struct Hit {
    title: String,
    url: String,
    snippet: String,
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

/// Value of `key=` inside a tag's attr source (quoted or bare).
fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let lower = attrs.to_lowercase();
    let i = lower.find(&format!("{key}="))? + key.len() + 1;
    let rest = &attrs[i..];
    if let Some(r) = rest.strip_prefix('"').or_else(|| rest.strip_prefix('\'')) {
        let q = attrs.as_bytes()[i] as char;
        r.find(q).map(|e| &r[..e])
    } else {
        rest.split(|c: char| c.is_whitespace() || c == '>')
            .next()
            .filter(|s| !s.is_empty())
    }
}

/// `<a …>inner</a>` occurrences where attrs contain `mark`.
fn anchors_with<'a>(html: &'a str, mark: &str) -> Vec<(&'a str, &'a str)> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = b[i..].iter().position(|c| *c == b'<') {
        let j = i + p;
        if b.get(j + 1) != Some(&b'a') || !matches!(b.get(j + 2), Some(b' ') | Some(b'\t')) {
            i = j + 1;
            continue;
        }
        let tag_end = match b[j..].iter().position(|c| *c == b'>') {
            Some(e) => j + e,
            None => break,
        };
        let attrs = &html[j..tag_end];
        if attrs.to_lowercase().contains(mark) {
            let close = html[tag_end..]
                .find("</a")
                .map(|e| tag_end + e)
                .unwrap_or(tag_end);
            out.push((attrs, &html[tag_end + 1..close.min(html.len())]));
        }
        i = tag_end + 1;
    }
    out
}

/// DDG wraps result hrefs as //duckduckgo.com/l/?uddg=<enc>&rut=… — unwrap.
fn unwrap_ddg(href: &str) -> String {
    let href = decode_entities(href.trim().trim_matches('"'));
    if let Some(p) = href.find("uddg=") {
        let enc = &href[p + 5..];
        let end = enc.find('&').unwrap_or(enc.len());
        return url_decode(&enc[..end]);
    }
    if let Some(h) = href.strip_prefix("//") {
        return format!("https://{h}");
    }
    href
}

fn search_ddg(query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", url_encode(query));
    let (status, html) = get(&url)?;
    if let Some(code) = status {
        if code >= 400 {
            return Err(format!("ddg: HTTP {code}"));
        }
    }
    let mut hits: Vec<Hit> = Vec::new();
    for (attrs, inner) in anchors_with(&html, "result__a") {
        let Some(href) = attr(attrs, "href") else {
            continue;
        };
        hits.push(Hit {
            title: decode_entities(strip_tags(inner).trim()),
            url: unwrap_ddg(href),
            snippet: String::new(),
        });
    }
    for (_, inner) in anchors_with(&html, "result__snippet") {
        if let Some(last) = hits.iter_mut().rev().find(|h| h.snippet.is_empty()) {
            last.snippet = decode_entities(strip_tags(inner).trim());
        }
    }
    hits.retain(|h| !h.title.is_empty() && h.url.starts_with("http"));
    hits.truncate(limit);
    if hits.is_empty() {
        Err(format!(
            "ddg: no results (HTTP {})",
            status.map(|c| c.to_string()).unwrap_or("?".into())
        ))
    } else {
        Ok(hits)
    }
}

fn instances() -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    if let Ok(u) = std::env::var("SEARXNG_URL") {
        let u = u.trim().trim_end_matches('/').to_string();
        if !u.is_empty() {
            list.push(u);
        }
    }
    match std::fs::read_to_string(state_dir().join("instances.txt")) {
        Ok(text) => {
            for l in text.lines() {
                let l = l.trim().trim_end_matches('/');
                if !l.is_empty() && !l.starts_with('#') {
                    list.push(l.to_string());
                }
            }
        }
        Err(_) => list.extend(DEFAULT_INSTANCES.iter().map(|s| s.to_string())),
    }
    list
}

/// Persisted round-robin cursor over `instances()`.
fn rr_cursor(n: usize) -> usize {
    std::fs::read_to_string(state_dir().join("rr"))
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|i| i % n.max(1))
        .unwrap_or(0)
}

fn rr_advance(n: usize, cur: usize) {
    if n == 0 {
        return;
    }
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(dir.join("rr"), ((cur + 1) % n).to_string()));
}

fn search_searxng(query: &str, limit: usize, start: usize) -> Result<(Vec<Hit>, usize), String> {
    let inst = instances();
    if inst.is_empty() {
        return Err("searxng: no instances configured".into());
    }
    let mut errs = Vec::new();
    for off in 0..inst.len() {
        let idx = (start + off) % inst.len();
        let base = &inst[idx];
        let url = format!("{base}/search?q={}&format=json", url_encode(query));
        match get(&url) {
            Ok((status, body)) => {
                if let Some(code) = status {
                    if code >= 400 {
                        errs.push(format!("{base}: HTTP {code}"));
                        continue;
                    }
                }
                let Ok(v) = serde_json::from_str::<Value>(&body) else {
                    errs.push(format!("{base}: non-JSON reply"));
                    continue;
                };
                let mut hits: Vec<Hit> = v
                    .get("results")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| {
                                Some(Hit {
                                    title: r.get("title")?.as_str()?.to_string(),
                                    url: r.get("url")?.as_str()?.to_string(),
                                    snippet: r
                                        .get("content")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                hits.retain(|h| !h.title.is_empty() && h.url.starts_with("http"));
                hits.truncate(limit);
                if hits.is_empty() {
                    errs.push(format!("{base}: no results"));
                    continue;
                }
                return Ok((hits, idx));
            }
            Err(e) => errs.push(format!("{base}: {e}")),
        }
    }
    Err(format!("searxng: {}", errs.join("; ")))
}

fn format_hits(hits: &[Hit]) -> String {
    let mut out = String::new();
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!("{}. {} — {}\n", i + 1, h.title, h.url));
        if !h.snippet.is_empty() {
            out.push_str(&h.snippet);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

fn call_tool(name: &str, args: &Value) -> Result<String, String> {
    if name != "web_query" {
        return Err(format!("unknown tool: {name}"));
    }
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if query.is_empty() {
        return Err("missing required argument: query".into());
    }
    let backend = args
        .get("backend")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, 25))
        .unwrap_or(DEFAULT_LIMIT);
    let n_inst = instances().len();
    let start = rr_cursor(n_inst);
    match backend {
        "ddg" | "ddgs" => search_ddg(query, limit).map(|h| format_hits(&h)),
        "searxng" => search_searxng(query, limit, start).map(|(h, i)| {
            rr_advance(n_inst, i);
            format_hits(&h)
        }),
        "auto" => match search_ddg(query, limit) {
            Ok(h) => Ok(format_hits(&h)),
            Err(ddg_err) => search_searxng(query, limit, start)
                .map(|(h, i)| {
                    rr_advance(n_inst, i);
                    format_hits(&h)
                })
                .map_err(|sx_err| format!("all backends failed — {ddg_err}; {sx_err}")),
        },
        other => Err(format!("unknown backend: {other} — auto|ddg|searxng")),
    }
}

/// One request → `Some(reply)`, or `None` for notifications. The bool asks
/// the loop to exit after writing the reply.
fn handle(req: &Value) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest(),
        "tool/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("args").cloned().unwrap_or(Value::Null);
            match call_tool(name, &args) {
                Ok(text) => json!({ "content": text }),
                Err(text) => json!({ "content": text, "is_error": true }),
            }
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest());
        return Ok(());
    }
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (reply, exit) = handle(&req);
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
        if exit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str, params: Value) -> Value {
        handle(&json!({ "id": 1, "method": method, "params": params }))
            .0
            .unwrap()
    }

    #[test]
    fn manifest_names_web_query() {
        let m = call("plugin/manifest", Value::Null)["result"].clone();
        assert_eq!(m["name"], "web");
        assert_eq!(m["tools"][0]["name"], "web_query");
        assert_eq!(m["tools"][0]["parameters"]["required"], json!(["query"]));
    }

    #[test]
    fn url_codec_roundtrips() {
        assert_eq!(url_encode("a b+c&d"), "a%20b%2Bc%26d");
        assert_eq!(url_decode("a%20b%2Bc+d"), "a b+c d");
        assert_eq!(url_decode(&url_encode("héllo wörld")), "héllo wörld");
    }

    #[test]
    fn ddg_redirects_unwrap() {
        assert_eq!(
            unwrap_ddg("//duckduckgo.com/l/?uddg=https%3A%2F%2Fex.com%2Fa%3Fb%3D1&rut=abc"),
            "https://ex.com/a?b=1"
        );
        assert_eq!(unwrap_ddg("https://ex.com/x"), "https://ex.com/x");
    }

    const DDG_HTML: &str = r##"<div class="result results_links">
        <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fone.example%2Fp&rut=xx">Title &amp; One</a>
        <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fone.example%2Fp&rut=xx">first snippet</a>
        <a class="result__a" href="https://two.example/">Two</a>
        <a class="result__snippet">second <b>snippet</b></a></div>"##;

    #[test]
    fn ddg_html_parses() {
        let mut hits = Vec::new();
        for (attrs, inner) in anchors_with(DDG_HTML, "result__a") {
            let href = attr(attrs, "href").unwrap();
            hits.push(Hit {
                title: decode_entities(strip_tags(inner).trim()),
                url: unwrap_ddg(href),
                snippet: String::new(),
            });
        }
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Title & One");
        assert_eq!(hits[0].url, "https://one.example/p");
        assert_eq!(hits[1].url, "https://two.example/");
    }

    #[test]
    fn searxng_json_shape_parses() {
        let body = json!({"results": [
            {"title": "T1", "url": "https://a.b", "content": "snip"},
            {"title": "T2", "url": "https://c.d"}
        ]});
        let hits: Vec<Hit> = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| {
                Some(Hit {
                    title: r.get("title")?.as_str()?.to_string(),
                    url: r.get("url")?.as_str()?.to_string(),
                    snippet: r
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                })
            })
            .collect();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            format_hits(&hits),
            "1. T1 — https://a.b\nsnip\n2. T2 — https://c.d"
        );
    }

    #[test]
    fn bad_args_are_tool_errors() {
        let r = call("tool/call", json!({ "name": "web_query", "args": {} }));
        assert_eq!(r["result"]["is_error"], true);
        let r = call(
            "tool/call",
            json!({ "name": "web_query", "args": { "query": "x", "backend": "nope" } }),
        );
        assert!(
            r["result"]["content"]
                .as_str()
                .unwrap()
                .contains("unknown backend")
        );
    }

    #[test]
    fn unknown_methods_are_method_not_found() {
        assert_eq!(call("nope", Value::Null)["error"]["code"], -32601);
    }

    #[test]
    fn shutdown_replies_then_exits() {
        let (reply, exit) = handle(&json!({ "id": 2, "method": "plugin/shutdown" }));
        assert!(reply.is_some() && exit);
    }
}
