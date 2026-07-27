//! The oracle's public HTTP surface — hardened, std-only, framework-free.
//!
//! Everything the outside world sees goes through here: /health, the
//! versioned /v1 API (with the permanent legacy aliases /api/feed and
//! /feed.json), the /share + /og social-card routes, /sitemap.xml and the
//! static dashboard. Thread-per-connection with read/write timeouts and a
//! global connection cap is all this workload needs — the default build
//! stays tokio-free.
//!
//! Handlers never re-serialize: build() publishes pre-serialized JSON into
//! `PubState` once per round and every request just clones an `Arc<str>`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_CONNS: usize = 256; // global cap — over it, immediate 503 + close
// 3s, not 5: we now wait for the whole header block, and holding 256 threads
// open on a half-sent request is a mild slowloris amplifier at MAX_CONNS.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const BUILD_FRESH_MS: u64 = 5_000; // /health flips to 503 when the last build is older
/// Per-IP OG card rate limit (abuse guard for expensive PNG renders).
/// 120, not 30: the bucket is a real client IP now (see `request_head` +
/// `behind_proxy`) instead of one shared 127.0.0.1 key for all of Caddy, and one
/// NAT'd office or Discord unfurl fan-out legitimately blows past 30/min.
const OG_RATE_PER_MIN: u32 = 120;
/// Hard cap on distinct rate-limit keys held at once (see `og_rate_ok`).
const OG_RATE_MAX_KEYS: usize = 10_000;

fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 }

/// Everything build() publishes, pre-serialized. Handlers only clone Arcs —
/// the per-request `serde_json` re-parse of the whole envelope is gone.
pub struct PubState {
    pub envelope: Mutex<Arc<str>>,                  // full /v1/feed JSON
    pub per_pair: Mutex<HashMap<String, Arc<str>>>, // "KAS-USD" -> FeedObj JSON
    pub catalog: Mutex<Arc<str>>,                   // light /v1/feeds JSON
    pub committee: Mutex<Arc<str>>,                 // /v1/committee pin artifact
    pub pairs: Mutex<Vec<String>>,                  // dash-form keys, for /sitemap.xml
    pub started: Instant,
    pub last_build_ms: AtomicU64, // epoch ms of the last publish
    pub round: AtomicU64,
    pub pools: AtomicUsize,
    pub feeds_total: AtomicUsize,
    pub feeds_live: AtomicUsize,
    /// simple request counters for /health metrics
    pub hits: AtomicU64,
    pub og_hits: AtomicU64,
}

impl PubState {
    pub fn new() -> Self {
        PubState {
            envelope: Mutex::new(Arc::from(r#"{"feeds":[]}"#)),
            per_pair: Mutex::new(HashMap::new()),
            catalog: Mutex::new(Arc::from(r#"{"round":0,"timestamp":0,"count":0,"feeds":[]}"#)),
            committee: Mutex::new(Arc::from(r#"{"threshold":3,"num_nodes":5,"signers":[]}"#)),
            pairs: Mutex::new(Vec::new()),
            started: Instant::now(),
            last_build_ms: AtomicU64::new(0),
            round: AtomicU64::new(0),
            pools: AtomicUsize::new(0),
            feeds_total: AtomicUsize::new(0),
            feeds_live: AtomicUsize::new(0),
            hits: AtomicU64::new(0),
            og_hits: AtomicU64::new(0),
        }
    }
    /// One call per build round — swaps in the new pre-serialized snapshot.
    pub fn publish(&self, envelope: String, per_pair: Vec<(String, String)>, catalog: String, committee: String,
                   round: u64, pools: usize, feeds_total: usize, feeds_live: usize) {
        *self.envelope.lock().unwrap() = Arc::from(envelope.as_str());
        {
            let mut pp = self.per_pair.lock().unwrap();
            pp.clear();
            let mut keys = Vec::with_capacity(per_pair.len());
            for (k, v) in per_pair { keys.push(k.clone()); pp.insert(k, Arc::from(v.as_str())); }
            *self.pairs.lock().unwrap() = keys;
        }
        *self.catalog.lock().unwrap() = Arc::from(catalog.as_str());
        *self.committee.lock().unwrap() = Arc::from(committee.as_str());
        self.round.store(round, Ordering::Relaxed);
        self.pools.store(pools, Ordering::Relaxed);
        self.feeds_total.store(feeds_total, Ordering::Relaxed);
        self.feeds_live.store(feeds_live, Ordering::Relaxed);
        self.last_build_ms.store(now_ms(), Ordering::Relaxed);
    }
}

/// BASE_URL (absolute origin for share/og/sitemap), trailing slash trimmed.
/// Unset => sitemap 404s and /share uses a relative og:image path.
fn base_url() -> Option<&'static str> {
    static BASE: OnceLock<Option<String>> = OnceLock::new();
    BASE.get_or_init(|| std::env::var("BASE_URL").ok()
        .map(|s| s.trim_end_matches('/').to_string()).filter(|s| !s.is_empty()))
        .as_deref()
}

// ---------- formatting (shared with the og module) ----------
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 { out.push(','); }
        out.push(c);
    }
    out
}
/// Human price: "118,235" / "4.52" / "0.0824" — 3 significant digits below $1
/// (tiny KRC-20 prices keep their leading zeros: "0.00000000324").
pub(crate) fn fmt_price(p: f64) -> String {
    if !p.is_finite() || p <= 0.0 { return "0".into(); }
    if p >= 1000.0 { thousands(p.round() as u64) }
    else if p >= 1.0 { format!("{p:.2}") }
    else {
        let d = ((-(p.log10().floor())) as usize + 2).min(14);
        format!("{p:.d$}")
    }
}
pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
/// Dash-form pair keys are discovery-sanitized to this charset (see
/// `clean_symbol` in main.rs); anything else can never name a feed, so reject
/// it before it reaches an HTML/JS/XML context rather than escaping per-context.
fn safe_key(k: &str) -> bool {
    !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
}

// ---------- server ----------
pub fn run(port: u16, state: Arc<PubState>) -> std::io::Result<()> {
    // KASPULSE_BIND=127.0.0.1 keeps the oracle loopback-only behind a local
    // reverse proxy (the VPS deploy); default stays 0.0.0.0.
    let bind = std::env::var("KASPULSE_BIND").unwrap_or_else(|_| "0.0.0.0".into());
    let listener = TcpListener::bind((bind.as_str(), port))?;
    let conns = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        if conns.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            conns.fetch_sub(1, Ordering::SeqCst);
            let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
            let _ = s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        let (state, conns) = (state.clone(), conns.clone());
        std::thread::spawn(move || {
            handle(s, &state);
            conns.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

/// Read the request line AND the header block (bodies are still irrelevant).
/// We need the headers because behind a reverse proxy every socket peer is
/// 127.0.0.1 — without X-Forwarded-For the OG limiter buckets the entire
/// internet as one client. Stops at the end of the header block or 8192
/// bytes, whichever comes first; a read error/timeout mid-way is not fatal as
/// long as we already have a request line (HTTP/1.0 clients that send no
/// blank line still get served).
fn request_head(s: &mut TcpStream) -> Option<(String, String, Option<String>)> {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 1024];
    loop {
        let n = match s.read(&mut tmp) { Ok(0) | Err(_) => break, Ok(n) => n };
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 { break; }
    }
    parse_head(&String::from_utf8_lossy(&buf))
}

/// Split off from the socket read so it can be tested directly.
fn parse_head(text: &str) -> Option<(String, String, Option<String>)> {
    let mut lines = text.lines();
    let line = lines.next()?;
    let mut it = line.split_whitespace();
    let method = it.next()?.to_string();
    let path = it.next()?.split('?').next().unwrap_or("/").to_string();
    // LAST comma-separated XFF entry, not the first. pulse.kascov.io is
    // directly internet-facing with no CDN, so Caddy APPENDS the true peer to
    // whatever XFF the client sent — the first entry is fully attacker-
    // controlled and would let anyone mint a fresh bucket per request.
    let mut xff = None;
    for l in lines {
        if l.is_empty() { break } // end of the header block; body (if any) is not headers
        let Some((k, v)) = l.split_once(':') else { continue };
        if !k.trim().eq_ignore_ascii_case("x-forwarded-for") { continue }
        // last header wins too (multiple XFF headers concatenate semantically)
        xff = v.rsplit(',').next().map(str::trim).filter(|e| ip_like(e)).map(str::to_string);
    }
    Some((method, path, xff))
}

/// Cheap shape check on an XFF entry before it becomes a HashMap key —
/// keeps a hostile header from stuffing 8KB of junk into the limiter map.
fn ip_like(s: &str) -> bool {
    !s.is_empty() && s.len() <= 45
        && s.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b'.' | b':' | b'%'))
}

/// Only trust X-Forwarded-For when we're bound to loopback — then the only
/// thing that can open a socket to us is the local reverse proxy. A 0.0.0.0
/// build (dev, or a direct deploy) must keep keying on the socket peer, or
/// every client can simply forge its own rate-limit bucket.
fn behind_proxy() -> bool {
    static P: OnceLock<bool> = OnceLock::new();
    *P.get_or_init(|| std::env::var("KASPULSE_BIND").map(|b| b.starts_with("127.")).unwrap_or(false))
}

fn send(s: &mut TcpStream, status: &str, ctype: &str, cache: &str, head_only: bool, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nAccess-Control-Allow-Origin: *\r\nX-Content-Type-Options: nosniff\r\nContent-Type: {ctype}\r\nCache-Control: {cache}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len());
    let _ = s.write_all(head.as_bytes());
    if !head_only { let _ = s.write_all(body); }
}
fn send_json(s: &mut TcpStream, status: &str, head: bool, body: &str) {
    send(s, status, "application/json", "no-store", head, body.as_bytes());
}

fn handle(mut s: TcpStream, st: &PubState) {
    let _ = s.set_read_timeout(Some(READ_TIMEOUT));
    let _ = s.set_write_timeout(Some(WRITE_TIMEOUT));
    let Some((method, path, xff)) = request_head(&mut s) else { return };
    let peer = match xff {
        Some(ip) if behind_proxy() => ip,
        _ => s.peer_addr().ok().map(|a| a.ip().to_string()).unwrap_or_default(),
    };
    st.hits.fetch_add(1, Ordering::Relaxed);
    match method.as_str() {
        "OPTIONS" => { // preflight, any path
            let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Max-Age: 3600\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        "GET" | "HEAD" => {}
        _ => return send_json(&mut s, "405 Method Not Allowed", false, r#"{"error":"method not allowed"}"#),
    }
    let head = method == "HEAD";
    match path.as_str() {
        "/health" => health(&mut s, st, head),
        "/v1/feed" | "/api/feed" | "/feed.json" => { // aliases are permanent
            let body = st.envelope.lock().unwrap().clone();
            send_json(&mut s, "200 OK", head, &body);
        }
        "/v1/feeds" => {
            let body = st.catalog.lock().unwrap().clone();
            send_json(&mut s, "200 OK", head, &body);
        }
        "/v1/committee" | "/api/committee" => {
            let body = st.committee.lock().unwrap().clone();
            // committee is pin-worthy — allow short cache so clients can hold it
            send(&mut s, "200 OK", "application/json", "public, max-age=60", head, body.as_bytes());
        }
        "/metrics" => metrics(&mut s, st, head),
        "/sitemap.xml" => sitemap(&mut s, st, head),
        p if p.starts_with("/v1/feed/") => pair_feed(&mut s, st, head, &p[9..]),
        p if p.starts_with("/api/feed/") => pair_feed(&mut s, st, head, &p[10..]),
        p if p.starts_with("/share/") => share(&mut s, st, head, &p[7..]),
        p if p.starts_with("/og/") && p.ends_with(".png") => og_card(&mut s, st, head, &p[4..p.len() - 4], &peer),
        _ => static_file(&mut s, head, &path),
    }
}

fn metrics(s: &mut TcpStream, st: &PubState, head: bool) {
    let body = format!(
        r#"{{"round":{},"uptime_s":{},"feeds_total":{},"feeds_live":{},"pools":{},"hits":{},"og_hits":{},"build_age_ms":{}}}"#,
        st.round.load(Ordering::Relaxed),
        st.started.elapsed().as_secs(),
        st.feeds_total.load(Ordering::Relaxed),
        st.feeds_live.load(Ordering::Relaxed),
        st.pools.load(Ordering::Relaxed),
        st.hits.load(Ordering::Relaxed),
        st.og_hits.load(Ordering::Relaxed),
        now_ms().saturating_sub(st.last_build_ms.load(Ordering::Relaxed)),
    );
    send_json(s, "200 OK", head, &body);
}

fn og_rate_ok(peer: &str) -> bool {
    static OG_HITS: OnceLock<Mutex<HashMap<String, (u64, u32)>>> = OnceLock::new();
    let map = OG_HITS.get_or_init(|| Mutex::new(HashMap::new()));
    let minute = now_ms() / 60_000;
    let mut g = map.lock().unwrap();
    // Entries are never removed on their own, so a wide scan (or a spoofed
    // XFF before we knew better) grows the map forever — drop everything not
    // in the current minute once it gets big. O(n) at most once per key past
    // the cap, and stale entries are worthless anyway.
    if g.len() > OG_RATE_MAX_KEYS { g.retain(|_, v| v.0 == minute); }
    let e = g.entry(peer.to_string()).or_insert((minute, 0));
    if e.0 != minute { *e = (minute, 0); }
    e.1 += 1;
    e.1 <= OG_RATE_PER_MIN
}

fn health(s: &mut TcpStream, st: &PubState, head: bool) {
    let age = now_ms().saturating_sub(st.last_build_ms.load(Ordering::Relaxed));
    let total = st.feeds_total.load(Ordering::Relaxed);
    let live = st.feeds_live.load(Ordering::Relaxed);
    let ok = age < BUILD_FRESH_MS && total >= 1 && live >= 1;
    let body = format!(
        r#"{{"ok":{ok},"round":{},"uptime_s":{},"build_age_ms":{age},"feeds_total":{total},"feeds_live":{live},"pools":{}}}"#,
        st.round.load(Ordering::Relaxed), st.started.elapsed().as_secs(), st.pools.load(Ordering::Relaxed));
    send_json(s, if ok { "200 OK" } else { "503 Service Unavailable" }, head, &body);
}

fn pair_feed(s: &mut TcpStream, st: &PubState, head: bool, seg: &str) {
    // PAIR is dash form, case-insensitive: kas-usd -> "KAS-USD"
    let key = seg.to_uppercase();
    let obj = st.per_pair.lock().unwrap().get(&key).cloned();
    match obj {
        Some(body) => send_json(s, "200 OK", head, &body),
        None => send_json(s, "404 Not Found", head, r#"{"error":"no such feed"}"#),
    }
}

/// Crawler-visible OG-meta page; humans get JS-redirected to the SPA route.
fn share(s: &mut TcpStream, st: &PubState, head: bool, seg: &str) {
    let key = seg.to_uppercase();
    // `key` is interpolated below into an attribute and a JS string — a key
    // that isn't charset-safe can't be a feed anyway, so 404 it here.
    if !safe_key(&key) { return send_json(s, "404 Not Found", head, r#"{"error":"no such feed"}"#) }
    let obj = st.per_pair.lock().unwrap().get(&key).cloned();
    let Some(json) = obj else { return send_json(s, "404 Not Found", head, r#"{"error":"no such feed"}"#) };
    // one small memoized-by-CDN parse per share hit is fine (cached 60s)
    let v: serde_json::Value = match serde_json::from_str(&json) { Ok(v) => v, Err(_) => return send_json(s, "404 Not Found", head, r#"{"error":"no such feed"}"#) };
    let pair = esc(v["pair"].as_str().unwrap_or(&key));
    let price = fmt_price(v["price"].as_f64().unwrap_or(0.0));
    let n = v["num_sources"].as_u64().unwrap_or(0);
    let base = base_url().unwrap_or("");
    let title = format!("{pair} ${price} — kaspulse");
    let desc = format!("median of {n} venues · 3-of-5 threshold-signed · verifiable by anyone");
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{title}</title>
<meta property="og:title" content="{title}">
<meta property="og:description" content="{desc}">
<meta property="og:image" content="{base}/og/{key}.png">
<meta property="og:type" content="website">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{title}">
<meta name="twitter:description" content="{desc}">
<meta name="twitter:image" content="{base}/og/{key}.png">
</head><body>
<script>location.replace('/#/feed/{key}')</script>
<noscript><a href="/#/feed/{key}">{pair} on kaspulse</a></noscript>
</body></html>
"#);
    send(s, "200 OK", "text/html; charset=utf-8", "public, max-age=60", head, html.as_bytes());
}

fn og_card(s: &mut TcpStream, st: &PubState, head: bool, seg: &str, peer: &str) {
    let key = seg.to_uppercase();
    if !st.per_pair.lock().unwrap().contains_key(&key) {
        return send_json(s, "404 Not Found", head, r#"{"error":"no such feed"}"#);
    }
    if !og_rate_ok(peer) {
        return send_json(s, "429 Too Many Requests", head, r#"{"error":"og rate limit"}"#);
    }
    st.og_hits.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "og")]
    {
        match crate::og::render_png(&key, st) {
            // 120s: unfurl bots and browsers should re-fetch a card at most
            // twice a minute-ish when a launch post spreads it around.
            Some(png) => send(s, "200 OK", "image/png", "public, max-age=120", head, &png),
            None => send_json(s, "404 Not Found", head,
                r#"{"error":"og cards unavailable: fonts missing — run scripts/fetch-fonts.sh"}"#),
        }
    }
    #[cfg(not(feature = "og"))]
    send_json(s, "404 Not Found", head, r#"{"error":"og cards not compiled into this build (cargo feature `og`)"}"#);
}

fn sitemap(s: &mut TcpStream, st: &PubState, head: bool) {
    let Some(base) = base_url() else { return send(s, "404 Not Found", "text/plain", "no-store", head, b"not found") };
    let pairs = st.pairs.lock().unwrap().clone();
    // esc() also covers XML: a symbol like "M&M" must not emit a bare '&'
    // into <loc> or crawlers reject the whole sitemap on first parse error.
    let urls: String = pairs.iter().filter(|p| safe_key(p))
        .map(|p| format!("<url><loc>{base}/share/{}</loc></url>", esc(p))).collect();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{urls}</urlset>\n");
    send(s, "200 OK", "application/xml", "public, max-age=300", head, xml.as_bytes());
}

// ---------- static files ----------
fn mime(p: &str) -> &'static str {
    let p = p.to_ascii_lowercase();
    if p.ends_with(".html") { "text/html; charset=utf-8" }
    else if p.ends_with(".js") || p.ends_with(".mjs") { "application/javascript" }
    else if p.ends_with(".css") { "text/css" }
    else if p.ends_with(".json") { "application/json" }
    else if p.ends_with(".png") { "image/png" }
    else if p.ends_with(".svg") { "image/svg+xml" }
    else if p.ends_with(".xml") { "application/xml" }
    else if p.ends_with(".ico") { "image/x-icon" }
    else if p.ends_with(".woff2") { "font/woff2" }
    else { "text/plain" }
}
fn static_file(s: &mut TcpStream, head: bool, path: &str) {
    let rel = if path == "/" { "index.html" } else { path.trim_start_matches('/') };
    // canonicalize BOTH the web root and the joined path, then require the
    // result to stay under the root — closes traversal (and symlink escapes)
    // properly, unlike the old substring '..' check.
    let nf = |s: &mut TcpStream| send(s, "404 Not Found", "text/plain", "no-store", head, b"not found");
    let Ok(root) = std::fs::canonicalize("web") else { return nf(s) };
    let Ok(full) = std::fs::canonicalize(root.join(rel)) else { return nf(s) };
    if !full.starts_with(&root) || !full.is_file() { return nf(s); }
    let Ok(body) = std::fs::read(&full) else { return nf(s) };
    let html = rel.ends_with(".html");
    // OG/Twitter scrapers require ABSOLUTE image URLs — the static pages ship
    // relative `content="/og…"` paths, so absolutize them here once BASE_URL
    // is known (same source of truth as /share and /sitemap.xml).
    let body = match base_url() {
        Some(base) if html => String::from_utf8_lossy(&body)
            .replace("content=\"/og", &format!("content=\"{base}/og")).into_bytes(),
        // robots.txt Sitemap directives must be absolute URLs or crawlers ignore them
        Some(base) if rel == "robots.txt" => String::from_utf8_lossy(&body)
            .replace("Sitemap: /sitemap.xml", &format!("Sitemap: {base}/sitemap.xml")).into_bytes(),
        _ => body,
    };
    let cache = if html { "no-cache" } else { "public, max-age=300" };
    send(s, "200 OK", mime(rel), cache, head, &body);
}

#[cfg(test)]
mod tests {
    use super::parse_head;

    fn xff(h: &str) -> Option<String> { parse_head(h).unwrap().2 }

    #[test]
    fn head_parses_and_takes_last_xff_entry() {
        // Caddy appends the true peer, so the LAST entry is the only one the
        // client could not choose. First-entry would be attacker-controlled.
        let h = "GET /og/KAS-USD.png?p=1 HTTP/1.1\r\nHost: x\r\nX-Forwarded-For: 1.2.3.4, 5.6.7.8\r\n\r\n";
        let (m, p, ip) = parse_head(h).unwrap();
        assert_eq!((m.as_str(), p.as_str()), ("GET", "/og/KAS-USD.png"));
        assert_eq!(ip.as_deref(), Some("5.6.7.8"));
    }

    #[test]
    fn xff_header_name_is_case_insensitive_and_last_header_wins() {
        assert_eq!(xff("GET / HTTP/1.1\r\nx-FORWARDED-for: 9.9.9.9\r\n\r\n").as_deref(), Some("9.9.9.9"));
        assert_eq!(xff("GET / HTTP/1.1\r\nX-Forwarded-For: 1.1.1.1\r\nX-Forwarded-For: 2.2.2.2\r\n\r\n").as_deref(),
                   Some("2.2.2.2"));
    }

    #[test]
    fn junk_and_missing_xff_fall_back_to_the_socket_peer() {
        assert_eq!(xff("GET / HTTP/1.1\r\nHost: x\r\n\r\n"), None);
        assert_eq!(xff("GET / HTTP/1.1\r\nX-Forwarded-For: \r\n\r\n"), None);
        // an unbounded junk key would be a memory-growth lever on the limiter
        assert_eq!(xff(&format!("GET / HTTP/1.1\r\nX-Forwarded-For: {}\r\n\r\n", "A".repeat(4000))), None);
        assert_eq!(xff("GET / HTTP/1.1\r\nX-Forwarded-For: not an ip\r\n\r\n"), None);
    }

    #[test]
    fn body_bytes_are_never_scanned_for_headers() {
        assert_eq!(xff("POST / HTTP/1.1\r\nHost: x\r\n\r\nX-Forwarded-For: 6.6.6.6\r\n"), None);
    }

    #[test]
    fn ipv6_survives_the_shape_check() {
        assert_eq!(xff("GET / HTTP/1.1\r\nX-Forwarded-For: 1.2.3.4, 2001:db8::ff00:42:8329\r\n\r\n").as_deref(),
                   Some("2001:db8::ff00:42:8329"));
    }
}
