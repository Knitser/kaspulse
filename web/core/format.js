// format.js — shared formatting + the ONE honest-status sentence.
// STATUS is canonical (frozen v1 contract §"Shared copy constants");
// guide.html and README.md make the same claim, never a stronger one.
'use strict';

export const STATUS =
  'The oracle, the signatures and every number on this page are live and real — ' +
  'verify one yourself below. On-chain consumers (price gates, slashing) are ' +
  'proven on Kaspa testnet-10; mainnet publishing is next.';

export const AS_OF = 'as of July 2026';

export const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

// group an integer string with thousands separators
const group = (s) => s.replace(/\B(?=(\d{3})+(?!\d))/g, ',');

/** Exact, significant-digit-aware price from the signed mant × 10^expo —
 *  no float rounding, works from BTC down to a $3e-9 meme token. */
export function fmtPrice(mant, expo) {
  let s = BigInt(mant).toString();
  const e = Number(expo);
  if (e >= 0) return '$' + group(s + '0'.repeat(e));
  const intLen = s.length + e;
  let int = intLen > 0 ? s.slice(0, intLen) : '0';
  let frac = (intLen > 0 ? s.slice(intLen) : '0'.repeat(-intLen) + s).replace(/0+$/, '');
  if (frac === '') return '$' + group(int);
  if (int !== '0') {
    // ≥ $1: cap at 2–4 decimals depending on magnitude
    frac = frac.slice(0, int.length >= 4 ? 2 : 4).replace(/0+$/, '');
    return '$' + group(int) + (frac ? '.' + frac : '');
  }
  // < $1: keep the leading zeros, cap at 4 significant digits
  const lead = frac.match(/^0*/)[0].length;
  frac = frac.slice(0, lead + 4).replace(/0+$/, '');
  return '$0.' + frac;
}

/** Same visual rules for a plain f64 (the /v1/feeds catalog has no mant/expo). */
export function fmtUsd(p) {
  p = Number(p);
  if (!isFinite(p) || p <= 0) return '$0';
  if (p >= 1000) return '$' + p.toLocaleString('en-US', { maximumFractionDigits: 2 });
  if (p >= 1) return '$' + p.toFixed(3).replace(/0+$/, '').replace(/\.$/, '');
  const lead = -Math.floor(Math.log10(p)) - 1;      // zeros after the point
  return '$' + p.toFixed(lead + 4).replace(/0+$/, '');
}

/** Human age from milliseconds: "412ms", "3.2s", "4m", "2h". */
export function ago(ms) {
  ms = Number(ms);
  if (!isFinite(ms) || ms < 0) return '—';
  if (ms < 1000) return ms.toFixed(0) + 'ms';
  if (ms < 60_000) return (ms / 1000).toFixed(1) + 's';
  if (ms < 3_600_000) return Math.floor(ms / 60_000) + 'm';
  return Math.floor(ms / 3_600_000) + 'h';
}

/** Age of a unix-seconds timestamp, e.g. signed_ts. */
export function agoTs(ts) { return ago(Date.now() - Number(ts) * 1000); }

export function bps(x) { return Number(x).toFixed(1) + ' bps'; }

/** "KAS/USD" → "KAS-USD" (the dash form the API routes use), and back. */
export const dash = (pair) => String(pair).replace('/', '-');
export const undash = (p) => String(p).replace('-', '/');
