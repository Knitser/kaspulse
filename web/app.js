'use strict';
const $ = (s) => document.querySelector(s);
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const last = {};            // pair -> last price (for flashing)
let latest = null;          // last full payload

function fmt(p) {
  p = Number(p);
  if (p >= 1000) return '$' + p.toLocaleString('en-US', { maximumFractionDigits: 2 });
  if (p >= 1) return '$' + p.toFixed(3);
  if (p >= 0.001) return '$' + p.toFixed(5);
  return '$' + p.toPrecision(3);   // tiny KRC-20 prices
}

async function poll() {
  try {
    const d = await (await fetch('/api/feed', { cache: 'no-cache' })).json();
    if (d && d.feeds) { latest = d; render(d); }
  } catch (e) { $('#live').classList.add('stale'); }
}

function render(d) {
  $('#round').textContent = 'round ' + d.round;
  $('#threshold').textContent = `${d.threshold}-of-${d.num_nodes} signed`;
  const majors = d.feeds.filter((f) => f.kind === 'major' && f.freshest_ms != null);
  const fastest = majors.length ? Math.min(...majors.map((f) => f.freshest_ms)) : null;
  const fs = $('#fresh-stat');
  fs.textContent = fastest == null ? '—' : fastest < 1000 ? `⚡ ${fastest}ms fresh` : `${(fastest / 1000).toFixed(1)}s`;
  fs.className = 'fresh-stat' + (fastest != null && fastest < 1000 ? ' fast' : '');
  $('#live').classList.remove('stale');
  $('#feeds').innerHTML = d.feeds.map(cardHtml).join('');
  d.feeds.forEach((f) => { last[f.pair] = f.price; });
  $('#boardfoot').innerHTML = `${d.feeds.length} feeds · majors stream over <b>${d.transport || 'websocket'}</b> (sub-second) + REST, median-signed by ${d.num_nodes} nodes. <span class="dim">Click a feed for sources &amp; timing.</span>`;
  document.querySelectorAll('.feed-card').forEach((c) => (c.onclick = () => detail(c.dataset.pair)));
}

function freshBadge(ms) {
  if (ms == null) return '';
  const fast = ms < 1000;
  const txt = fast ? `⚡ ${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
  return `<span class="fc-fresh ${fast ? 'fast' : 'slow'}">${txt}</span>`;
}
function cardHtml(f) {
  const prev = last[f.pair];
  const dir = prev == null ? '' : f.price > prev ? 'up' : f.price < prev ? 'dn' : '';
  const tag = f.kind === 'krc20' ? '<span class="fc-tag krc20">KRC-20</span>' : '<span class="fc-tag major">major</span>';
  const feat = f.pair === 'KAS/USD' ? ' featured' : '';
  return `<div class="feed-card${feat}" data-pair="${esc(f.pair)}">
    <div class="fc-top"><span class="fc-pair">${esc(f.pair)}</span><span class="fc-tophead">${freshBadge(f.freshest_ms)}${tag}</span></div>
    <div class="fc-price ${dir}">${fmt(f.price)}</div>
    ${spark(f.history)}
    <div class="fc-foot"><span>${f.num_sources} source${f.num_sources === 1 ? '' : 's'}</span><span class="fc-ok">✓ ${f.threshold}-of-${f.signatures.length} signed</span></div>
  </div>`;
}

function spark(hist) {
  const W = 300, H = 46, pad = 3;
  if (!hist || hist.length < 2) return `<svg class="fc-spark" viewBox="0 0 ${W} ${H}"></svg>`;
  const ps = hist.map((h) => h[1]), lo = Math.min(...ps), hi = Math.max(...ps), rng = hi - lo || 1;
  const x = (i) => pad + (i / (hist.length - 1)) * (W - 2 * pad);
  const y = (p) => pad + (1 - (p - lo) / rng) * (H - 2 * pad);
  const pts = hist.map((h, i) => `${x(i).toFixed(1)},${y(h[1]).toFixed(1)}`).join(' ');
  const up = ps[ps.length - 1] >= ps[0];
  const col = up ? 'var(--green)' : 'var(--red)';
  return `<svg class="fc-spark" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none"><polyline points="${pts}" fill="none" stroke="${col}" stroke-width="1.6" stroke-linejoin="round"/></svg>`;
}

// click a feed → sources + the signed attestation
const modal = $('#modal');
$('#modal-x').onclick = () => (modal.hidden = true);
modal.onclick = (e) => { if (e.target === modal) modal.hidden = true; };
function detail(pair) {
  const f = latest.feeds.find((x) => x.pair === pair);
  if (!f) return;
  modal.hidden = false;
  $('#modal-body').innerHTML = `
    <div class="m-eyebrow">${f.kind === 'krc20' ? 'KRC-20 token' : 'major'} · signed feed</div>
    <div class="m-title">${esc(f.pair)} ${fmt(f.price)}</div>
    <p class="m-body">Median of ${f.num_sources} venue${f.num_sources === 1 ? '' : 's'} · freshest ${f.freshest_ms}ms · spread ${Number(f.spread_bps).toFixed(1)} bps · signed by ${f.signatures.length} nodes (need ${f.threshold}).</p>
    <div class="d-sources">${f.sources.map((s) => {
      const isMed = Math.abs(s.price - f.median) < 1e-12;
      const age = s.age_ms < 1000 ? `${s.age_ms}ms` : `${(s.age_ms / 1000).toFixed(1)}s`;
      return `<div class="d-src ${isMed ? 'med' : ''}"><span>${esc(s.name)}${isMed ? ' <span class="d-medtag">median</span>' : ''}</span><span class="mono">${fmt(s.price)} <span class="d-age ${s.age_ms < 1500 ? 'fresh' : ''}">${age}</span></span></div>`;
    }).join('')}</div>
    <div class="d-sig">
      <div class="d-sig-k">signed message</div><div class="mono d-sig-v">${esc(f.message)}</div>
      <div class="d-sig-k">signature (node 0)</div><div class="mono d-sig-v">${esc(f.signatures[0].slice(0, 40))}…</div>
    </div>
    <p class="m-alt">verify it yourself: <span class="mono">cargo run --bin verify</span> — re-checks every signature + re-fetches the market.</p>`;
}

const line = () => {};   // noop (kept for parity)
poll();
setInterval(poll, 1500);
