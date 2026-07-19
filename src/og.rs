//! OG share cards: /og/{PAIR}.png — a 1200×630 snapshot of one feed, rendered
//! server-side from an SVG (resvg), so a shared link shows the live price, the
//! sparkline and the trust line instead of a blank embed.
//!
//! Compiled only with `--features og` (the Dockerfile enables it); the default
//! build stays resvg-free. Fonts are loaded at RUNTIME from assets/fonts/
//! (JetBrains Mono Regular + Bold, fetched by scripts/fetch-fonts.sh — the
//! repo never vendors font binaries). If the fonts are absent the renderer
//! returns None, /og 404s with a note, and nothing else is affected.
//!
//! Cost bound: a 5s per-pair memo + `Cache-Control: public, max-age=60` on
//! the route — no rate-limiter machinery needed.

use crate::http::{esc, fmt_price, PubState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const W: f64 = 1200.0; // card is 1200×630 (the og:image standard)
const MEMO_MS: u128 = 5_000;

static MEMO: OnceLock<Mutex<HashMap<String, (Instant, Vec<u8>)>>> = OnceLock::new();
static FONTDB: OnceLock<Option<Arc<resvg::usvg::fontdb::Database>>> = OnceLock::new();

/// Load the two TTFs once. None (and one loud line) when they're absent —
/// run scripts/fetch-fonts.sh to enable cards.
fn fontdb() -> Option<Arc<resvg::usvg::fontdb::Database>> {
    FONTDB
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            let mut loaded = 0;
            for f in ["JetBrainsMono-Regular.ttf", "JetBrainsMono-Bold.ttf"] {
                match std::fs::read(format!("assets/fonts/{f}")) {
                    Ok(bytes) => { db.load_font_data(bytes); loaded += 1; }
                    Err(_) => {}
                }
            }
            if loaded < 2 {
                eprintln!("og: assets/fonts/JetBrainsMono-{{Regular,Bold}}.ttf missing — /og cards disabled (run scripts/fetch-fonts.sh)");
                return None;
            }
            Some(Arc::new(db))
        })
        .clone()
}

/// Render (or serve the ≤5s-old memoized) card for a dash-form pair key that
/// the caller has already confirmed exists in `state.per_pair`.
pub fn render_png(key: &str, state: &PubState) -> Option<Vec<u8>> {
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((t, png)) = memo.lock().unwrap().get(key) {
        if t.elapsed().as_millis() < MEMO_MS { return Some(png.clone()); }
    }
    let db = fontdb()?;
    let json = state.per_pair.lock().unwrap().get(key)?.clone();
    // a serde parse here is fine — it's behind the 5s memo, not per-request
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let svg = card_svg(&v);
    let png = rasterize(&svg, db)?;
    memo.lock().unwrap().insert(key.to_string(), (Instant::now(), png.clone()));
    Some(png)
}

fn hhmmss(ts: u64) -> String {
    let t = ts % 86_400;
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

/// history [[ts,price],...] -> polyline points inside the given box.
fn sparkline(hist: &[serde_json::Value], x0: f64, y0: f64, w: f64, h: f64) -> String {
    let pts: Vec<(f64, f64)> = hist.iter().filter_map(|e| {
        let a = e.as_array()?;
        Some((a.first()?.as_u64()? as f64, a.get(1)?.as_f64()?))
    }).collect();
    if pts.len() < 2 { return String::new(); }
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for (_, p) in &pts { lo = lo.min(*p); hi = hi.max(*p); }
    let span = if hi - lo > 0.0 { hi - lo } else { 1.0 };
    let n = (pts.len() - 1) as f64;
    let s: Vec<String> = pts.iter().enumerate().map(|(i, (_, p))| {
        let x = x0 + (i as f64 / n) * w;
        let y = if hi - lo > 0.0 { y0 + h - ((p - lo) / span) * h } else { y0 + h / 2.0 };
        format!("{x:.1},{y:.1}")
    }).collect();
    let last = s.last().cloned().unwrap_or_default();
    let (lx, ly) = last.split_once(',').unwrap_or(("0", "0"));
    format!(
        r##"<polyline points="{}" fill="none" stroke="#c792ea" stroke-width="3" stroke-linejoin="round" stroke-linecap="round" opacity="0.9"/><circle cx="{lx}" cy="{ly}" r="6" fill="#49eacb"/>"##,
        s.join(" "))
}

/// The card: kaspulse palette (#080b11 ground, teal price, violet sparkline),
/// thin/halted badge rendered ON the card so a screenshot of a thin pool
/// never circulates as an unqualified price.
fn card_svg(v: &serde_json::Value) -> String {
    let pair = esc(v["pair"].as_str().unwrap_or("?"));
    let price = format!("${}", fmt_price(v["price"].as_f64().unwrap_or(0.0)));
    let n = v["num_sources"].as_u64().unwrap_or(0);
    let thin = v["thin"].as_bool().unwrap_or(false);
    let halted = v["halted"].as_bool().unwrap_or(false);
    let signed_ts = v["signed_ts"].as_u64().unwrap_or(0);
    let empty = Vec::new();
    let hist = v["history"].as_array().unwrap_or(&empty);
    let trust = format!("median of {n} venues · 3-of-5 signed · signed {} UTC · kaspulse", hhmmss(signed_ts));
    // price auto-fit: JetBrains Mono Bold ≈ 0.6em advance; keep inside ~1000px
    let price_size = (1000.0 / (0.6 * price.len() as f64)).min(96.0);
    let badge = if halted { Some(("#ff5370", "HALTED")) }
        else if thin { Some(("#f5c518", "THIN LIQUIDITY")) } else { None };
    let badge_svg = badge.map(|(col, label)| {
        let w = 46.0 + label.len() as f64 * 12.5;
        let x = 1128.0 - w;
        format!(
            r#"<g><rect x="{x:.0}" y="56" width="{w:.0}" height="44" rx="22" fill="none" stroke="{col}" stroke-opacity="0.6" stroke-width="1.5"/><circle cx="{:.0}" cy="78" r="5.5" fill="{col}"/><text x="{:.0}" y="85" font-family="JetBrains Mono" font-size="19" letter-spacing="2" fill="{col}">{label}</text></g>"#,
            x + 24.0, x + 40.0)
    }).unwrap_or_default();
    let spark = sparkline(hist, 72.0, 432.0, W - 144.0, 130.0);
    format!(
        r##"<svg width="1200" height="630" viewBox="0 0 1200 630" xmlns="http://www.w3.org/2000/svg">
<defs>
<radialGradient id="glow" cx="0.5" cy="-0.19" r="1"><stop offset="0" stop-color="#49eacb" stop-opacity="0.13"/><stop offset="0.7" stop-color="#49eacb" stop-opacity="0"/></radialGradient>
<pattern id="grid" width="48" height="48" patternUnits="userSpaceOnUse"><path d="M 48 0 L 0 0 0 48" fill="none" stroke="#49eacb" stroke-opacity="0.045" stroke-width="1"/></pattern>
</defs>
<rect width="1200" height="630" fill="#080b11"/>
<rect width="1200" height="630" fill="url(#grid)"/>
<rect width="1200" height="630" fill="url(#glow)"/>
<g transform="translate(72,62)"><circle cx="14" cy="14" r="11" fill="none" stroke="#49eacb" stroke-width="3.5"/><circle cx="14" cy="14" r="4" fill="#49eacb"/></g>
<text x="112" y="87" font-family="JetBrains Mono" font-weight="700" font-size="32" fill="#e8edf2">kaspulse</text>
{badge_svg}
<text x="72" y="212" font-family="JetBrains Mono" font-weight="700" font-size="68" letter-spacing="-1" fill="#e8edf2">{pair}</text>
<text x="72" y="330" font-family="JetBrains Mono" font-weight="700" font-size="{price_size:.0}" fill="#49eacb">{price}</text>
<text x="72" y="392" font-family="JetBrains Mono" font-size="27" fill="#8b93a7">{trust}</text>
{spark}
<line x1="72" y1="588" x2="1128" y2="588" stroke="#1a2130" stroke-width="1"/>
</svg>"##)
}

/// SVG string -> PNG bytes at intrinsic size (same pipeline as kascov's og.rs).
fn rasterize(svg: &str, db: Arc<resvg::usvg::fontdb::Database>) -> Option<Vec<u8>> {
    let opt = resvg::usvg::Options { fontdb: db, ..Default::default() };
    let tree = resvg::usvg::Tree::from_str(svg, &opt).ok()?;
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}
