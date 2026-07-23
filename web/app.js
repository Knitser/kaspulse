// app.js — kaspulse SPA. No build step, no framework: core/router.js parses
// the hash, core/api.js polls the frozen /v1 surface at 1.5s, and the views
// below render into <main id="view">. The verify button talks to
// window.kaspulseVerify (web/vendor/verify.js, loaded before this module).
'use strict';

import { startRouter } from './core/router.js';
import { pollCatalog, pollFeed, fetchEnvelope, fetchFeed } from './core/api.js';
import { STATUS, AS_OF, esc, fmtPrice, fmtUsd, ago, agoTs, bps, dash, undash } from './core/format.js';

const view = document.getElementById('view');
const lastPx = {};            // pair -> last price, for direction flashes
let stops = [];               // active pollers, stopped on route change

const stopAll = () => { stops.forEach((s) => s()); stops = []; };
const $ = (sel, el = document) => el.querySelector(sel);

function toast(msg) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(t._t);
  t._t = setTimeout(() => (t.hidden = true), 3200);
}

function flashDir(pair, price) {
  const prev = lastPx[pair];
  lastPx[pair] = price;
  return prev == null || price === prev ? '' : price > prev ? 'up' : 'dn';
}

function sparkSvg(hist, cls = 'fc-spark', W = 300, H = 46) {
  if (!hist || hist.length < 2) return `<svg class="${cls}" viewBox="0 0 ${W} ${H}"></svg>`;
  const pad = 3, ps = hist.map((h) => h[1]);
  const lo = Math.min(...ps), hi = Math.max(...ps), rng = hi - lo || 1;
  const x = (i) => pad + (i / (hist.length - 1)) * (W - 2 * pad);
  const y = (p) => pad + (1 - (p - lo) / rng) * (H - 2 * pad);
  const pts = hist.map((h, i) => `${x(i).toFixed(1)},${y(h[1]).toFixed(1)}`).join(' ');
  const col = ps[ps.length - 1] >= ps[0] ? 'var(--green)' : 'var(--red)';
  return `<svg class="${cls}" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none"><polyline points="${pts}" fill="none" stroke="${col}" stroke-width="1.6" stroke-linejoin="round"/></svg>`;
}

function badges(f) {
  const b = [];
  if (f.halted) b.push('<span class="badge halted">⏸ halted</span>');
  if (f.thin) b.push('<span class="badge thin">thin</span>');
  if (f.degraded) b.push('<span class="badge degraded">degraded</span>');
  if (f.peg_ok === false) b.push('<span class="badge depeg">depeg</span>');
  if (f.outliers && f.outliers.length) b.push(`<span class="badge outlier">outlier: ${esc(f.outliers.join(','))}</span>`);
  return b.join(' ');
}

function freshDot(ms) {
  const cls = ms == null ? '' : ms < 1000 ? 'fast' : ms < 10_000 ? '' : 'slow';
  return `<span class="fresh-dot ${cls}"></span>`;
}

function catalogCard(row, extraSpark = '') {
  const dir = flashDir(row.pair, row.price);
  const tag = row.kind === 'krc20' ? '<span class="fc-tag krc20">KRC-20</span>' : '<span class="fc-tag major">major</span>';
  return `<div class="feed-card" data-pair="${esc(dash(row.pair))}">
    <div class="fc-top"><span class="fc-pair">${esc(row.pair)}</span><span class="fc-tophead">${badges(row)}${tag}</span></div>
    <div class="fc-price ${dir}">${fmtUsd(row.price)}</div>${extraSpark}
    <div class="fc-foot"><span>${freshDot(row.freshest_ms)}${ago(row.freshest_ms)} · ${row.num_sources} source${row.num_sources === 1 ? '' : 's'}</span><span>${bps(row.spread_bps)}</span></div>
  </div>`;
}

const wireCards = (root) => root.querySelectorAll('.feed-card[data-pair]').forEach((c) => (c.onclick = () => (location.hash = '#/feed/' + c.dataset.pair)));

/* ---------- in-browser verification (the flagship moment) ---------- */

async function runVerify(outEl, pairDash, btn) {
  const kv = window.kaspulseVerify;
  if (!kv || !kv.ready) {
    outEl.innerHTML = '<div class="v-unavail">verifier unavailable in this browser</div>';
    return;
  }
  if (btn) btn.disabled = true;
  outEl.innerHTML = '<div class="v-sub mono">fetching the signed feed…</div>';
  let feed, r;
  try {
    feed = await fetchFeed(pairDash);
    r = kv.verifyFeed(feed);
  } catch (e) {
    outEl.innerHTML = `<div class="v-banner bad">could not fetch ${esc(pairDash)} — ${esc(e.message || e)}</div>`;
    if (btn) btn.disabled = false;
    return;
  }
  const rows = (r.results || []).map((n, i) =>
    `<div class="v-row ${n.ok ? 'ok' : 'bad'}"><span>node ${i}</span><span class="v-hex">${esc(String(n.signer).slice(0, 16))}…</span><span class="v-mark">${n.ok ? '✓ signature valid' : '✗ signature FAILED'}</span></div>`);
  rows.push(`<div class="v-row ${r.bound ? 'ok' : 'bad'}"><span>binding</span><span class="v-hex">message fields ↔ JSON fields</span><span class="v-mark">${r.bound ? '✓ bound' : '✗ MISMATCH'}</span></div>`);
  outEl.innerHTML = `<div class="verify-rows">${rows.join('')}</div><div class="v-after"></div>`;
  const els = outEl.querySelectorAll('.v-row');
  els.forEach((el, i) => setTimeout(() => el.classList.add('show'), 120 + i * 250));
  setTimeout(() => {
    const after = $('.v-after', outEl);
    if (r.ok) {
      after.innerHTML = `<div class="v-banner">${r.threshold}-of-${(r.results || []).length} verified · you just re-checked the committee&rsquo;s math yourself</div>
        <div class="v-sub">screenshot this. that&rsquo;s the point.</div>
        <div class="v-note">for the truly paranoid: <span class="mono">cargo run --bin verify</span> re-fetches the exchanges themselves and cross-checks our median too.</div>`;
    } else {
      after.innerHTML = `<div class="v-banner bad">verification FAILED — ${r.valid} of ${(r.results || []).length} signatures valid (need ${r.threshold})${r.bound ? '' : ' · signed message does not match the JSON fields'}${r.error ? ' · ' + esc(r.error) : ''}</div>
        <div class="v-sub">this is the verifier doing its job: if the data lies, the math says so.</div>`;
    }
    if (btn) btn.disabled = false;
  }, 120 + els.length * 250 + 250);
}

/* ---------- landing ---------- */

function renderLanding() {
  view.innerHTML = `
  <section class="hero">
    <div class="eyebrow">the covenant-ready price oracle · Kaspa L1</div>
    <h1>Every price Kaspa needs,<br><span class="grad">signed &amp; verifiable.</span></h1>
    <p class="lede">Since Toccata, a Kaspa coin can refuse to move unless the price is right. The chain can&rsquo;t see prices — so kaspulse signs them where covenants can: 60+ live feeds, majors under a second, plus 55+ KRC-20 tokens nobody else carries. Median of independent venues, 3-of-5 threshold-signed, verifiable by anyone. Including you. Including a coin.</p>
    <div class="stat-row" id="hero-stats"><span>connecting…</span></div>
  </section>

  <div class="honest"><div class="honest-in">${esc(STATUS)}</div></div>

  <section class="sec">
    <div style="display:flex;justify-content:space-between;align-items:baseline;flex-wrap:wrap;gap:8px">
      <h2 class="section-h">Live board</h2>
      <a class="more-link" href="#/feeds" id="all-feeds-link">all feeds →</a>
    </div>
    <div class="feeds" id="teaser"></div>
  </section>

  <section class="sec" style="text-align:center">
    <h2 class="section-h">Don&rsquo;t trust this page</h2>
    <p class="section-sub" style="margin-left:auto;margin-right:auto">Everything above is just JSON from our API — you shouldn&rsquo;t believe it. So don&rsquo;t: this button re-verifies the committee&rsquo;s Schnorr signatures over the exact signed message, in your browser, against the five node keys the feed itself carries. If a single digit didn&rsquo;t match what those keys signed, the math would fail. (Honest limit: your browser learns the keys from the same response — pin the committee keys from a second channel for full independence.)</p>
    <div class="verify-box">
      <button class="btn" id="hero-verify-btn">verify KAS/USD in my browser</button>
      <div id="hero-verify-out"></div>
    </div>
  </section>

  <section class="sec">
    <h2 class="section-h">How it reaches a covenant</h2>
    <div class="steps">
      <div class="step"><div class="step-k">measured</div><p>seven exchanges + 55+ DEX pools, MAD-filtered median, circuit breakers — no single venue can move a feed.</p></div>
      <div class="step"><div class="step-k">signed</div><p>5 independent nodes, 3 must agree — Schnorr over <span class="mono">kaspulse/v2|PAIR|mant|expo|ts|round</span>.</p></div>
      <div class="step"><div class="step-k">enforced</div><p>a covenant re-checks the signatures itself with <span class="mono">OpCheckSigFromStack</span> — L1 is the verifier, ~1.4s after the tick.</p></div>
    </div>
    <p class="steps-hook">What keeps nodes honest? A bond that gets slashed if a node ever signs two prices for one round. We slashed our own, on testnet, on purpose. <a href="/guide.html#honest">→</a></p>
  </section>

  <section class="sec">
    <h2 class="section-h">Two oracles on Kaspa. Different questions.</h2>
    <p class="section-sub">QUEX brings TEE-attested data to Igra&rsquo;s EVM; kaspulse brings threshold-signed prices to Kaspa&rsquo;s L1 covenants. If you&rsquo;re writing Solidity on Igra and want a push feed today, use it. If you want a Kaspa coin itself to enforce a price, that&rsquo;s the seat kaspulse sits in.</p>
    <div class="cmp-wrap"><table class="cmp">
      <tr><th></th><th>kaspulse</th><th>QUEX</th></tr>
      <tr><td>question</td><td class="us">what price can a Kaspa L1 covenant enforce?</td><td>what off-chain data can an EVM contract pull?</td></tr>
      <tr><td>runs on</td><td class="us">Kaspa L1 — covenants verify signatures at spend time</td><td>Igra L2 (EVM) — Solidity contracts</td></tr>
      <tr><td>trust model</td><td class="us">3-of-5 threshold signatures — anyone can re-run the math, in a browser</td><td>trusted execution (Intel TDX) — hardware-attested data path</td></tr>
      <tr><td>feeds</td><td class="us">KAS + majors + 56 KRC-20 pool prices nobody else carries</td><td>general-purpose — any HTTPS source a contract requests</td></tr>
      <tr><td>accountability</td><td class="us">bonded nodes; equivocation slashing demonstrated on testnet-10</td><td>enclave attestation by the hardware vendor</td></tr>
      <tr><td>status</td><td class="us">oracle live · L1 consumers on testnet-10</td><td>live on Igra mainnet</td></tr>
    </table></div>
    <div class="cmp-note">${esc(AS_OF)} — corrections welcome; being generous to QUEX here is deliberate.</div>
  </section>

  <section class="sec">
    <h2 class="section-h">Build on it</h2>
    <div class="dev-cards">
      <a class="dev-card" href="#/dev"><div class="dev-card-h">the API</div><p>Frozen /v1, no keys, CORS-open. Envelope, per-pair feeds, light catalog.</p><pre class="code">curl ${esc(location.origin)}/v1/feed/KAS-USD</pre></a>
      <a class="dev-card" href="#/dev"><div class="dev-card-h">the SDK</div><p>Rust: fetch + verify + covenant recipes, all proven bytes.</p><pre class="code">kaspulse-sdk = "0.2"
# git today — crates.io pending
let f = fetch(BASE, "KAS/USD")?;
f.checked_value_fresh(30s)?</pre></a>
      <a class="dev-card" href="/guide.html"><div class="dev-card-h">the covenant guide</div><p>Price gates, range settles, slashing — every step proven on testnet-10 with real txids.</p><pre class="code">price_gate_redeem(&amp;committee, strike_e8)</pre></a>
      <a class="dev-card" href="#/docs"><div class="dev-card-h">the verify tool</div><p>One file, zero deps, full cryptographic verification.</p><pre class="code">node kaspulse.mjs verify KAS/USD</pre></a>
    </div>
  </section>`;

  $('#hero-verify-btn').onclick = () => runVerify($('#hero-verify-out'), 'KAS-USD', $('#hero-verify-btn'));

  let movers = null;      // top-3 KRC-20 movers, computed once from the envelope
  let sparks = {};        // pair -> history (from the envelope, static seed)
  let catalog = null;

  fetchEnvelope().then((env) => {
    const chg = (f) => {
      if (!f.history || f.history.length < 2) return 0;
      const a = f.history[0][1], b = f.history[f.history.length - 1][1];
      return a ? Math.abs((b - a) / a) : 0;
    };
    movers = env.feeds.filter((f) => f.kind === 'krc20').sort((x, y) => chg(y) - chg(x)).slice(0, 3).map((f) => f.pair);
    env.feeds.forEach((f) => (sparks[f.pair] = f.history));
    if (catalog) paintTeaser(catalog);
  }).catch(() => {});

  function paintTeaser(c) {
    const rows = Object.fromEntries(c.feeds.map((r) => [r.pair, r]));
    const picks = ['BTC/USD', 'ETH/USD', ...(movers || c.feeds.filter((r) => r.kind === 'krc20').slice(0, 3).map((r) => r.pair))]
      .map((p) => rows[p]).filter(Boolean);
    const kas = rows['KAS/USD'];
    const feat = kas ? `<div class="feed-card featured" data-pair="KAS-USD">
        <div><div class="fc-top"><span class="fc-pair">KAS/USD</span><span class="fc-tophead">${badges(kas)}<span class="fc-tag major">major</span></span></div>
        <div class="fc-price ${flashDir('feat:KAS/USD', kas.price)}">${fmtUsd(kas.price)}</div>
        <div class="fc-foot"><span>${freshDot(kas.freshest_ms)}${ago(kas.freshest_ms)} · ${kas.num_sources} sources · ${bps(kas.spread_bps)}</span><span class="fc-ok">✓ 3-of-5 signed</span></div></div>
        <div id="feat-spark">${sparkSvg(sparks['KAS/USD'], 'fc-spark', 620, 72)}</div>
      </div>` : '';
    $('#teaser').innerHTML = feat + picks.map((r) => catalogCard(r, sparkSvg(sparks[r.pair]))).join('');
    wireCards($('#teaser'));
    $('#all-feeds-link').textContent = `all ${c.count} feeds →`;
  }

  stops.push(pollCatalog((c, err) => {
    const s = $('#hero-stats');
    if (!c) { if (s) s.innerHTML = '<span class="dim">reconnecting…</span>'; return; }
    catalog = c;
    const majors = c.feeds.filter((f) => f.kind === 'major').map((f) => f.freshest_ms);
    const freshest = majors.length ? Math.min(...majors) : null;
    if (s) s.innerHTML = `<span><b>${c.count}</b> feeds</span>${freshest == null ? '' : `<span class="${freshest < 1000 ? 'fast' : ''}">⚡ <b>${ago(freshest)}</b> freshest</span>`}<span>round <b>${c.round}</b></span><span><b>3-of-5</b> signed</span><span><b>1.39s</b> tick→L1 (measured)</span>`;
    paintTeaser(c);
  }));

  stops.push(pollFeed('KAS-USD', (f) => {
    if (!f) return;
    sparks['KAS/USD'] = f.history;
    const el = $('#feat-spark');
    if (el) el.innerHTML = sparkSvg(f.history, 'fc-spark', 620, 72);
  }));
}

/* ---------- #/feeds board ---------- */

function renderBoard() {
  view.innerHTML = `
  <section class="sec" style="margin-top:26px">
    <h2 class="section-h">All feeds</h2>
    <p class="section-sub" id="board-meta">connecting…</p>
    <div class="board-controls">
      <div class="tabbar" id="board-tabs">
        <button data-f="all" class="active">all</button>
        <button data-f="major">majors</button>
        <button data-f="krc20">KRC-20</button>
      </div>
      <input class="search" id="board-q" type="search" placeholder="search pairs…" autocomplete="off">
    </div>
    <div class="feeds" id="board-grid"></div>
  </section>`;

  let filter = 'all', q = '', catalog = null;
  const grid = $('#board-grid');

  function paint() {
    if (!catalog) return;
    const rows = catalog.feeds.filter((r) =>
      (filter === 'all' || r.kind === filter) &&
      (!q || r.pair.toLowerCase().includes(q)));
    grid.innerHTML = rows.length
      ? rows.map((r) => catalogCard(r)).join('')
      : '<div class="board-empty" style="grid-column:1/-1">no feeds match</div>';
    wireCards(grid);
    $('#board-meta').innerHTML = `${catalog.count} live feeds · round ${catalog.round} · every card is a median of independent venues, 3-of-5 signed — click one for sources, signatures and the verify button.`;
  }

  $('#board-tabs').onclick = (e) => {
    const b = e.target.closest('button'); if (!b) return;
    filter = b.dataset.f;
    $('#board-tabs').querySelectorAll('button').forEach((x) => x.classList.toggle('active', x === b));
    paint();
  };
  $('#board-q').oninput = (e) => { q = e.target.value.trim().toLowerCase(); paint(); };

  stops.push(pollCatalog((c) => { if (c) { catalog = c; paint(); } }));
}

/* ---------- #/feed/{PAIR} detail ---------- */

function renderFeedPage(pairDash) {
  view.innerHTML = '<section class="sec" style="margin-top:26px"><p class="dim mono">loading feed…</p></section>';
  let built = false;

  function build(f) {
    const pd = dash(f.pair);
    view.innerHTML = `
    <section class="sec" style="margin-top:26px">
      <div class="detail-head">
        <span class="detail-pair">${esc(f.pair)}</span>
        <span class="fc-tag ${f.kind === 'krc20' ? 'krc20' : 'major'}">${f.kind === 'krc20' ? 'KRC-20' : 'major'}</span>
        <span id="d-badges">${badges(f)}</span>
        <span class="badge fresh" id="d-fresh"></span>
      </div>
      <div id="d-halted"></div>
      <div class="detail-price" id="d-price"></div>
      <div id="d-thin"></div>
      <div class="stat-strip">
        <div class="stat"><b id="s-spread">—</b><span>spread</span></div>
        <div class="stat"><b id="s-range">—</b><span>low / high</span></div>
        <div class="stat"><b id="s-src">—</b><span>venues</span></div>
        <div class="stat" id="s-liq-wrap" hidden><b id="s-liq">—</b><span>liq (WKAS)</span></div>
        <div class="stat"><b id="s-round">—</b><span>signed round</span></div>
        <div class="stat"><b id="s-signed">—</b><span>signed</span></div>
      </div>
      <div class="detail-grid">
        <div class="panel">
          <div class="panel-h">Venues this round <span class="dim">(median highlighted)</span></div>
          <div class="d-sources" id="d-sources"></div>
          <div class="panel-h" style="margin-top:20px">History <span class="dim">(the in-feed ~120 points — all there is)</span></div>
          <div id="d-chart"></div>
        </div>
        <div class="panel">
          <div class="panel-h">Attestation — what the committee actually signed</div>
          <code class="att-msg mono" id="d-msg"></code>
          <div class="att-decode">the signed price is <span class="mono">mant × 10^expo</span> — exact at any magnitude, from BTC to a $3e-9 meme token</div>
          <div class="sig-list" id="d-sigs"></div>
          <div class="share-row">
            <button class="btn small" id="d-verify-btn">verify ${esc(f.pair)} in my browser</button>
            <button class="btn small" id="d-share-btn">share this feed</button>
          </div>
          <div id="d-verify-out"></div>
        </div>
      </div>
      <div class="panel" style="margin-top:18px">
        <div class="panel-h">Integrate — this pair, four ways</div>
        <div class="int-tabs" id="int-tabs">
          <div class="tabbar">
            <button data-tab="t-curl" class="active">curl</button>
            <button data-tab="t-rust">Rust</button>
            <button data-tab="t-js">JS</button>
            <button data-tab="t-cov">covenant</button>
          </div>
          <div class="tabpane active" id="t-curl"><div class="codebox"><pre class="code">curl ${esc(location.origin)}/v1/feed/${esc(pd)}</pre></div></div>
          <div class="tabpane" id="t-rust"><div class="codebox"><pre class="code">// kaspulse-sdk = { git = "https://github.com/Knitser/kaspulse", package = "kaspulse-sdk" }
let f = kaspulse_sdk::fetch("${esc(location.origin)}", "${esc(f.pair)}")?;
println!("{}", f.checked_value_fresh(Duration::from_secs(30))?);</pre></div></div>
          <div class="tabpane" id="t-js"><div class="codebox"><pre class="code">import { Kaspulse } from './kaspulse.mjs';
const k = new Kaspulse('${esc(location.origin)}');
const feed = await k.feed('${esc(f.pair)}');
if (!k.verifyFeed(feed).ok) throw new Error('bad feed');
console.log(k.checkedValue(feed));</pre></div></div>
          <div class="tabpane" id="t-cov"><div class="codebox"><pre class="code">// on-chain gate (demo committee — see the guide's honest labeling)
use kaspulse_sdk::covenant::{self, Prefix};
let redeem  = covenant::price_gate_redeem(&amp;committee, strike_e8);
let addr    = covenant::p2sh_address(&amp;redeem, Prefix::Testnet)?; // fund this
let witness = covenant::price_gate_witness(&amp;sigs, price_e8, &amp;redeem);</pre></div></div>
        </div>
      </div>
    </section>`;

    $('#int-tabs .tabbar').onclick = (e) => {
      const b = e.target.closest('button'); if (!b) return;
      $('#int-tabs .tabbar').querySelectorAll('button').forEach((x) => x.classList.toggle('active', x === b));
      $('#int-tabs').querySelectorAll('.tabpane').forEach((p) => p.classList.toggle('active', p.id === b.dataset.tab));
    };
    addCopyButtons(view);
    $('#d-verify-btn').onclick = () => runVerify($('#d-verify-out'), pd, $('#d-verify-btn'));
    $('#d-share-btn').onclick = async () => {
      const url = location.origin + '/share/' + pd;
      try { await navigator.clipboard.writeText(url); toast('copied — this link unfurls with a live price card'); }
      catch { toast(url); }
    };
  }

  function update(f) {
    const dir = flashDir('detail:' + f.pair, f.price);
    const price = $('#d-price');
    price.textContent = fmtPrice(f.mant, f.expo);
    price.className = 'detail-price ' + dir;
    $('#d-badges').innerHTML = badges(f);
    $('#d-fresh').textContent = 'freshest ' + ago(f.freshest_ms);
    $('#d-halted').innerHTML = f.halted ? '<div class="halted-banner">⏸ circuit breaker — this feed is halted; the committee is not signing fresh prices for it right now. Do not consume.</div>' : '';
    $('#d-thin').innerHTML = f.thin ? '<div class="thin-note">thin pool — the price is real but shallow; consumers should honor the <span class="mono">thin</span> flag.</div>' : '';
    $('#s-spread').textContent = bps(f.spread_bps);
    $('#s-range').textContent = `${fmtUsd(f.low)} / ${fmtUsd(f.high)}`;
    $('#s-src').textContent = f.num_sources;
    if (f.kind === 'krc20') { $('#s-liq-wrap').hidden = false; $('#s-liq').textContent = Math.round(f.liq_wkas).toLocaleString('en-US'); }
    $('#s-round').textContent = f.signed_round;
    $('#s-signed').textContent = agoTs(f.signed_ts) + ' ago';
    $('#d-sources').innerHTML = f.sources.map((s) => {
      const isMed = Math.abs(s.price - f.median) < 1e-12;
      return `<div class="d-src ${isMed ? 'med' : ''}"><span>${esc(s.name)}${isMed ? ' <span class="d-medtag">median</span>' : ''}</span><span class="mono">${fmtUsd(s.price)} <span class="d-age ${s.age_ms < 1500 ? 'fresh' : ''}">${ago(s.age_ms)}</span></span></div>`;
    }).join('');
    $('#d-chart').innerHTML = sparkSvg(f.history, 'chart', 640, 160);
    $('#d-msg').textContent = f.message;
    $('#d-sigs').innerHTML = f.signers.map((pk, i) =>
      `<div class="sig-item"><span class="s-k">node ${i}</span><span class="s-v" data-full-pk="${esc(pk)}" data-full-sig="${esc(f.signatures[i])}" title="click to expand">${esc(pk.slice(0, 20))}… · sig ${esc(f.signatures[i].slice(0, 20))}…</span></div>`).join('');
    $('#d-sigs').querySelectorAll('.s-v').forEach((el) => (el.onclick = () => {
      el.textContent = el.dataset.fullPk + ' · sig ' + el.dataset.fullSig;
      el.onclick = null;
    }));
  }

  stops.push(pollFeed(pairDash, (f, err) => {
    if (!f) {
      if (err === 404 && !built) {
        stopAll();
        view.innerHTML = `<div class="notfound"><p class="mono">404 — no such feed: ${esc(undash(pairDash))}</p><p><a class="more-link" href="#/feeds">browse all feeds →</a></p></div>`;
      }
      return;
    }
    if (!built) { build(f); built = true; }
    update(f);
  }));
}

/* ---------- #/dev + #/docs (template-driven) ---------- */

function subOrigin(root) {
  root.querySelectorAll('.code').forEach((el) => {
    const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n; (n = w.nextNode());) if (n.nodeValue.includes('$ORIGIN')) n.nodeValue = n.nodeValue.replaceAll('$ORIGIN', location.origin);
  });
}

function addCopyButtons(root) {
  root.querySelectorAll('.codebox').forEach((box) => {
    if (box.querySelector('.copy-btn')) return;
    const b = document.createElement('button');
    b.className = 'copy-btn';
    b.textContent = 'copy';
    b.onclick = async () => {
      try { await navigator.clipboard.writeText(box.querySelector('pre').textContent); b.textContent = 'copied'; }
      catch { b.textContent = 'failed'; }
      setTimeout(() => (b.textContent = 'copy'), 1500);
    };
    box.appendChild(b);
  });
}

function renderDev() {
  view.replaceChildren(document.getElementById('tpl-dev').content.cloneNode(true));
  subOrigin(view);
  addCopyButtons(view);
  // scroll-spy
  const nav = $('#api-nav');
  const links = [...nav.querySelectorAll('a[data-spy]')];
  links.forEach((a) => (a.onclick = (e) => {
    e.preventDefault();
    document.getElementById(a.dataset.spy)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }));
  view.querySelectorAll('[data-spy-link]').forEach((a) => (a.onclick = (e) => {
    e.preventDefault();
    document.getElementById(a.dataset.spyLink)?.scrollIntoView({ behavior: 'smooth' });
  }));
  const obs = new IntersectionObserver((entries) => {
    entries.forEach((en) => {
      if (!en.isIntersecting) return;
      links.forEach((a) => a.classList.toggle('active', a.dataset.spy === en.target.id));
    });
  }, { rootMargin: '0px 0px -70% 0px' });
  view.querySelectorAll('.api-endpoint').forEach((el) => obs.observe(el));
  stops.push(() => obs.disconnect());
}

function renderDocs() {
  view.replaceChildren(document.getElementById('tpl-docs').content.cloneNode(true));
  subOrigin(view);
  addCopyButtons(view);
  const bar = $('#qs-tabs .tabbar');
  bar.onclick = (e) => {
    const b = e.target.closest('button'); if (!b) return;
    bar.querySelectorAll('button').forEach((x) => x.classList.toggle('active', x === b));
    $('#qs-tabs').querySelectorAll('.tabpane').forEach((p) => p.classList.toggle('active', p.id === b.dataset.tab));
  };
  $('#docs-verify-btn').onclick = () => runVerify($('#docs-verify-out'), 'KAS-USD', $('#docs-verify-btn'));
}

/* ---------- boot ---------- */

document.getElementById('foot-status').textContent = STATUS + ' (' + AS_OF + '.)';

const views = { landing: renderLanding, feeds: renderBoard, feed: renderFeedPage, dev: renderDev, docs: renderDocs };

startRouter(({ route, param }) => {
  stopAll();
  window.scrollTo(0, 0);
  document.querySelectorAll('.nav-links a[data-nav]').forEach((a) => a.classList.toggle('active', a.dataset.nav === route));
  views[route](param);
});
