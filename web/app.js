'use strict';
const $ = (s) => document.querySelector(s);
const TICK = 2000;                 // matches the oracle's tick
let lastPrice = null, lastRound = -1, lastFetch = 0;

const fmtUsd = (p) => '$' + Number(p).toLocaleString('en-US', { minimumFractionDigits: 5, maximumFractionDigits: 6 });
const short = (h, n = 10) => (h ? h.slice(0, n) + '…' + h.slice(-6) : '—');

async function poll() {
  try {
    const feed = await (await fetch('/api/feed', { cache: 'no-cache' })).json();
    if (feed && feed.price) { render(feed); lastFetch = Date.now(); }
  } catch (e) { markStale(); }
}

function render(f) {
  $('#pair').textContent = f.pair || 'KAS/USD';

  // price + flash
  const el = $('#price');
  const cls = lastPrice == null ? '' : (f.price > lastPrice ? 'flash-up' : f.price < lastPrice ? 'flash-dn' : '');
  el.innerHTML = `<span class="${cls}">${fmtUsd(f.price)}</span>`;
  lastPrice = f.price;

  $('#round').textContent = 'round ' + f.round;
  $('#live').innerHTML = '<span class="on">●</span> LIVE';
  $('#live').classList.remove('stale');

  // sources
  const med = f.median;
  $('#sources').innerHTML = (f.sources || []).map((s) => {
    const isMed = Math.abs(s.price - med) < 1e-9;
    return `<div class="src ${isMed ? 'is-med' : ''}"><span class="src-n">${esc(s.name)}${isMed ? '<span class="src-tag">median</span>' : ''}</span><span class="src-p">${fmtUsd(s.price)}</span></div>`;
  }).join('');
  $('#median').textContent = fmtUsd(med);
  $('#spread').textContent = (f.spread_bps != null ? f.spread_bps.toFixed(1) : '—') + ' bps';
  $('#nsrc').textContent = f.num_sources ?? (f.sources ? f.sources.length : '—');

  // signers / threshold (handles single-node now, multi-node after decentralization)
  const signers = f.signers || (f.signer ? [f.signer] : []);
  const k = f.threshold || 1;
  $('#signers').textContent = signers.length + (signers.length === 1 ? ' node' : ' independent nodes');
  $('#threshold').textContent = `${k}-of-${signers.length}` + (signers.length === 1 ? ' (single — decentralize to remove trust)' : ' must agree');
  $('#message').textContent = f.message || '—';
  $('#signature').textContent = short(f.signatures ? f.signatures[0] : f.signature, 20);

  // on-chain status
  if (f.onchain && f.onchain.txid) {
    const oc = $('#onchain'); oc.classList.add('live-oc');
    $('#oc-text').innerHTML = `published on Kaspa TN10 · <a href="https://explorer-tn10.kaspa.org/txs/${f.onchain.txid}" target="_blank" rel="noopener">price coin ${short(f.onchain.txid, 8)}</a>`;
  }

  drawSpark(f.history || []);
}

function drawSpark(hist) {
  const svg = $('#spark'); const W = 600, H = 120, pad = 6;
  if (hist.length < 2) { svg.innerHTML = ''; return; }
  const ps = hist.map((h) => h[1]);
  const lo = Math.min(...ps), hi = Math.max(...ps), rng = hi - lo || 1;
  const x = (i) => pad + (i / (hist.length - 1)) * (W - 2 * pad);
  const y = (p) => pad + (1 - (p - lo) / rng) * (H - 2 * pad);
  const pts = hist.map((h, i) => `${x(i).toFixed(1)},${y(h[1]).toFixed(1)}`).join(' ');
  const area = `${pad},${H} ${pts} ${W - pad},${H}`;
  const up = hist[hist.length - 1][1] >= hist[0][1];
  const col = up ? 'var(--green)' : 'var(--red)';
  svg.innerHTML = `
    <defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="${col}" stop-opacity="0.25"/><stop offset="1" stop-color="${col}" stop-opacity="0"/>
    </linearGradient></defs>
    <polygon points="${area}" fill="url(#g)"/>
    <polyline points="${pts}" fill="none" stroke="${col}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>
    <circle cx="${x(hist.length - 1)}" cy="${y(hist[hist.length - 1][1])}" r="3.5" fill="${col}"/>`;
}

function markStale() {
  const l = $('#live'); if (l) { l.textContent = '● reconnecting'; l.classList.add('stale'); }
}

// "updated Xs ago" + next-update progress bar
setInterval(() => {
  if (!lastFetch) return;
  const age = (Date.now() - lastFetch) / 1000;
  $('#updated').textContent = age < 1.5 ? 'just now' : `updated ${age.toFixed(0)}s ago`;
  const pct = Math.min(100, ((Date.now() - lastFetch) / TICK) * 100);
  $('#nextbar').style.width = pct + '%';
  if (age > 8) markStale();
}, 200);

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

poll();
setInterval(poll, 1000);
