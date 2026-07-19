// api.js — 1.5s pollers against the frozen /v1 surface (no SSE by decision).
// Relative URLs only; single in-flight guard; pauses while the tab is hidden.
'use strict';

const TICK_MS = 1500;

function makePoller(url, cb) {
  let stopped = false, inflight = false, first = true;
  async function tick() {
    if (stopped || inflight || (document.hidden && !first)) return;
    first = false;
    inflight = true;
    try {
      const r = await fetch(url, { cache: 'no-store' });
      if (stopped) return;
      if (r.ok) cb(await r.json(), null);
      else cb(null, r.status);
    } catch (e) {
      if (!stopped) cb(null, e);
    } finally {
      inflight = false;
    }
  }
  tick();
  const timer = setInterval(tick, TICK_MS);
  const onVis = () => { if (!document.hidden) tick(); };   // catch up on return
  document.addEventListener('visibilitychange', onVis);
  return () => {   // stop()
    stopped = true;
    clearInterval(timer);
    document.removeEventListener('visibilitychange', onVis);
  };
}

/** Poll the light /v1/feeds catalog (what boards render). Returns stop(). */
export const pollCatalog = (cb) => makePoller('/v1/feeds', cb);

/** Poll one full FeedObj; pair in dash form (KAS-USD). Returns stop(). */
export const pollFeed = (pairDash, cb) =>
  makePoller('/v1/feed/' + encodeURIComponent(pairDash), cb);

/** One-shot full envelope (heavy — used once for movers/sparkline seeds). */
export async function fetchEnvelope() {
  const r = await fetch('/v1/feed', { cache: 'no-store' });
  if (!r.ok) throw new Error('envelope HTTP ' + r.status);
  return r.json();
}

/** One-shot single feed; throws {status:404} on unknown pair (real 404). */
export async function fetchFeed(pairDash) {
  const r = await fetch('/v1/feed/' + encodeURIComponent(pairDash), { cache: 'no-store' });
  if (!r.ok) { const e = new Error('HTTP ' + r.status); e.status = r.status; throw e; }
  return r.json();
}
