/* vendored from clients/js/kaspulse.mjs — keep in sync; run scripts/check-vendored.sh */
/* kaspulse in-browser verifier — classic script (no module) so it loads before
   app.js everywhere. Exposes window.kaspulseVerify per the frozen v1 contract:
   { ready, selfTest(), verifyFeed(feedObj) }. If the embedded self-tests fail,
   ready=false and the UI must show "verifier unavailable" — NEVER a fake ✓.
   The core block below is intentionally at column 0: check-vendored.sh diffs
   it byte-for-byte against the canonical copy in clients/js/kaspulse.mjs. */
(() => {
'use strict';
// CRYPTO-CORE-BEGIN
// ── shared crypto core ──────────────────────────────────────────────────────
// This exact block lives in clients/js/kaspulse.mjs (canonical) and
// web/vendor/verify.js (vendored copy). scripts/check-vendored.sh diffs the
// region byte-for-byte, so keep it dependency-free, export-free and at column 0.

// bytes / hex helpers (lowercase-hex canonical, tolerant of uppercase input)
function hexToBytes(hex) {
  if (typeof hex !== 'string' || hex.length % 2 !== 0 || /[^0-9a-fA-F]/.test(hex)) return null;
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}
function bytesToHex(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i++) s += bytes[i].toString(16).padStart(2, '0');
  return s;
}
function asciiBytes(s) { return new TextEncoder().encode(s); }
function bytesToBig(bytes) {
  let v = 0n;
  for (let i = 0; i < bytes.length; i++) v = (v << 8n) | BigInt(bytes[i]);
  return v;
}
function concatBytes(...parts) {
  let n = 0;
  for (const p of parts) n += p.length;
  const out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}

// ── blake2b-256 — pure BigInt, unkeyed, 32-byte digest ──────────────────────
// (adapted from kascov's proven web/blake2b.js — same author, same vectors)
const B2B_IV = [
  0x6a09e667f3bcc908n, 0xbb67ae8584caa73bn, 0x3c6ef372fe94f82bn, 0xa54ff53a5f1d36f1n,
  0x510e527fade682d1n, 0x9b05688c2b3e6c1fn, 0x1f83d9abfb41bd6bn, 0x5be0cd19137e2179n,
];
const B2B_SIGMA = [
  [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
  [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
  [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
  [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
  [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
  [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
  [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
  [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
  [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
  [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
  [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
  [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];
const B2B_M64 = (1n << 64n) - 1n;
function b2bRotr(x, n) { return ((x >> n) | (x << (64n - n))) & B2B_M64; }
function b2bCompress(h, block, t, last) {
  const m = new Array(16);
  for (let i = 0; i < 16; i++) {
    let w = 0n;
    for (let j = 7; j >= 0; j--) w = (w << 8n) | BigInt(block[i * 8 + j]);
    m[i] = w;
  }
  const v = h.concat(B2B_IV.slice());
  v[12] ^= BigInt(t) & B2B_M64;
  if (last) v[14] ^= B2B_M64;
  const G = (a, b, c, d, x, y) => {
    v[a] = (v[a] + v[b] + x) & B2B_M64; v[d] = b2bRotr(v[d] ^ v[a], 32n);
    v[c] = (v[c] + v[d]) & B2B_M64;     v[b] = b2bRotr(v[b] ^ v[c], 24n);
    v[a] = (v[a] + v[b] + y) & B2B_M64; v[d] = b2bRotr(v[d] ^ v[a], 16n);
    v[c] = (v[c] + v[d]) & B2B_M64;     v[b] = b2bRotr(v[b] ^ v[c], 63n);
  };
  for (let r = 0; r < 12; r++) {
    const s = B2B_SIGMA[r];
    G(0, 4, 8, 12, m[s[0]], m[s[1]]);  G(1, 5, 9, 13, m[s[2]], m[s[3]]);
    G(2, 6, 10, 14, m[s[4]], m[s[5]]); G(3, 7, 11, 15, m[s[6]], m[s[7]]);
    G(0, 5, 10, 15, m[s[8]], m[s[9]]); G(1, 6, 11, 12, m[s[10]], m[s[11]]);
    G(2, 7, 8, 13, m[s[12]], m[s[13]]); G(3, 4, 9, 14, m[s[14]], m[s[15]]);
  }
  for (let i = 0; i < 8; i++) h[i] = (h[i] ^ v[i] ^ v[i + 8]) & B2B_M64;
}
/* blake2b-256(bytes) -> Uint8Array(32); unkeyed, no salt/personalization */
function blake2b256(input) {
  const h = B2B_IV.slice();
  h[0] ^= 0x01010000n ^ 32n; // digest_length=32, fanout=1, depth=1
  let t = 0;
  let i = 0;
  // full blocks except the last (the final block is always compressed with last=true)
  while (input.length - i > 128) {
    t += 128;
    b2bCompress(h, input.subarray(i, i + 128), t, false);
    i += 128;
  }
  const block = new Uint8Array(128);
  block.set(input.subarray(i));
  t += input.length - i;
  b2bCompress(h, block, t, true);
  const out = new Uint8Array(32);
  for (let k = 0; k < 4; k++) {
    let w = h[k];
    for (let j = 0; j < 8; j++) { out[k * 8 + j] = Number(w & 0xffn); w >>= 8n; }
  }
  return out;
}

// ── sha256 — sync pure JS, needed only for BIP340's tagged hashes ───────────
// (sync avoids crypto.subtle's async/secure-context constraints)
const SHA_K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);
function sha256(data) {
  const bitLen = data.length * 8;
  const padded = new Uint8Array((((data.length + 8) >> 6) << 6) + 64);
  padded.set(data);
  padded[data.length] = 0x80;
  const dv = new DataView(padded.buffer);
  dv.setUint32(padded.length - 8, Math.floor(bitLen / 0x100000000));
  dv.setUint32(padded.length - 4, bitLen >>> 0);
  const H = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
  const w = new Uint32Array(64);
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  for (let i = 0; i < padded.length; i += 64) {
    for (let j = 0; j < 16; j++) w[j] = dv.getUint32(i + j * 4);
    for (let j = 16; j < 64; j++) {
      const s0 = rotr(w[j - 15], 7) ^ rotr(w[j - 15], 18) ^ (w[j - 15] >>> 3);
      const s1 = rotr(w[j - 2], 17) ^ rotr(w[j - 2], 19) ^ (w[j - 2] >>> 10);
      w[j] = (w[j - 16] + s0 + w[j - 7] + s1) >>> 0;
    }
    let a = H[0], b = H[1], c = H[2], d = H[3], e = H[4], f = H[5], g = H[6], h = H[7];
    for (let j = 0; j < 64; j++) {
      const t1 = (h + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + SHA_K[j] + w[j]) >>> 0;
      const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) >>> 0;
      h = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    H[0] = (H[0] + a) >>> 0; H[1] = (H[1] + b) >>> 0; H[2] = (H[2] + c) >>> 0; H[3] = (H[3] + d) >>> 0;
    H[4] = (H[4] + e) >>> 0; H[5] = (H[5] + f) >>> 0; H[6] = (H[6] + g) >>> 0; H[7] = (H[7] + h) >>> 0;
  }
  const out = new Uint8Array(32);
  const ov = new DataView(out.buffer);
  for (let j = 0; j < 8; j++) ov.setUint32(j * 4, H[j]);
  return out;
}
function taggedHash(tag, ...chunks) {
  const t = sha256(asciiBytes(tag));
  return sha256(concatBytes(t, t, ...chunks));
}

// ── BIP340 Schnorr verification over secp256k1 — BigInt, verify-only ────────
const SECP_P = 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2fn;
const SECP_N = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
const SECP_G = [
  0x79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798n,
  0x483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8n,
];
function fmod(a) { const r = a % SECP_P; return r < 0n ? r + SECP_P : r; }
function modInv(a, m) {
  let r0 = ((a % m) + m) % m, r1 = m, s0 = 1n, s1 = 0n;
  while (r1 !== 0n) {
    const q = r0 / r1;
    [r0, r1] = [r1, r0 - q * r1];
    [s0, s1] = [s1, s0 - q * s1];
  }
  return ((s0 % m) + m) % m;
}
function modPow(base, exp, m) {
  let b = ((base % m) + m) % m, e = exp, r = 1n;
  while (e > 0n) {
    if (e & 1n) r = (r * b) % m;
    b = (b * b) % m;
    e >>= 1n;
  }
  return r;
}
// affine points as [x, y]; null = point at infinity
function pointAdd(p, q) {
  if (p === null) return q;
  if (q === null) return p;
  const [x1, y1] = p, [x2, y2] = q;
  let lam;
  if (x1 === x2) {
    if (fmod(y1 + y2) === 0n) return null; // P + (−P)
    lam = fmod(3n * x1 * x1 * modInv(2n * y1, SECP_P)); // double
  } else {
    lam = fmod((y2 - y1) * modInv(x2 - x1, SECP_P));
  }
  const x3 = fmod(lam * lam - x1 - x2);
  return [x3, fmod(lam * (x1 - x3) - y1)];
}
// double-and-add scalar multiplication (verification of public data — no
// side-channel concern)
function pointMul(p, k) {
  let r = null, a = p, e = k;
  while (e > 0n) {
    if (e & 1n) r = pointAdd(r, a);
    a = pointAdd(a, a);
    e >>= 1n;
  }
  return r;
}
// BIP340 lift_x: the curve point with this x and EVEN y, or null
function liftX(x) {
  if (x >= SECP_P) return null;
  const c = fmod(x * x * x + 7n);
  const y = modPow(c, (SECP_P + 1n) / 4n, SECP_P);
  if ((y * y) % SECP_P !== c) return null;
  return [x, (y & 1n) === 0n ? y : SECP_P - y];
}
/* Standard BIP340 verification: pubkey32 = x-only key, msg32 = 32-byte message
   (for kaspulse: the blake2b-256 digest — NOT hashed again outside BIP340's own
   tagged hashing), sig64 = r||s. Returns bool, never throws. */
function bip340Verify(pubkey32, msg32, sig64) {
  if (!(pubkey32 instanceof Uint8Array) || pubkey32.length !== 32) return false;
  if (!(msg32 instanceof Uint8Array) || msg32.length !== 32) return false;
  if (!(sig64 instanceof Uint8Array) || sig64.length !== 64) return false;
  const P = liftX(bytesToBig(pubkey32));
  if (P === null) return false;
  const r = bytesToBig(sig64.subarray(0, 32));
  const s = bytesToBig(sig64.subarray(32));
  if (r >= SECP_P || s >= SECP_N) return false;
  const e = bytesToBig(taggedHash('BIP0340/challenge', sig64.subarray(0, 32), pubkey32, msg32)) % SECP_N;
  const R = pointAdd(pointMul(SECP_G, s), pointMul(P, SECP_N - e)); // s·G − e·P
  if (R === null || (R[1] & 1n) !== 0n || R[0] !== r) return false;
  return true;
}

// ── the kaspulse/v2 signed message and the feed verdict ─────────────────────
/* message = "kaspulse/v2|PAIR|mant|expo|ts|round" (ASCII, decimal integers,
   expo may be negative). Returns the five fields AS STRINGS (no float
   round-trips) or null if the shape is wrong. */
function parseSignedMessage(message) {
  if (typeof message !== 'string') return null;
  const parts = message.split('|');
  if (parts.length !== 6 || parts[0] !== 'kaspulse/v2') return null;
  const [, pair, mant, expo, ts, round] = parts;
  if (!pair || !/^\d+$/.test(mant) || !/^-?\d+$/.test(expo) || !/^\d+$/.test(ts) || !/^\d+$/.test(round)) return null;
  return { pair, mant, expo, ts, round };
}
/* Verify one FeedObj (as parsed from /v1/feed/{PAIR}). Pure sync, no network.
   VALID := (count of BIP340-verifying signatures ≥ threshold) AND the signed
   message's PAIR/mant/expo/ts equal the JSON's pair/mant/expo/signed_ts. */
function verifyFeedCore(feed) {
  if (!CORE_SELFTEST.ok) throw new Error('verifier self-test failed');
  if (!feed || typeof feed !== 'object' || typeof feed.message !== 'string') {
    return { ok: false, valid: 0, threshold: 0, bound: false, parsed: null, results: [], error: 'not a feed object (need message/signers/signatures)' };
  }
  const signers = Array.isArray(feed.signers) ? feed.signers : [];
  const sigs = Array.isArray(feed.signatures) ? feed.signatures : [];
  const threshold = Number.isInteger(feed.threshold) && feed.threshold > 0 ? feed.threshold : 0;
  const parsed = parseSignedMessage(feed.message);
  // field binding: what was SIGNED must equal what the JSON claims
  // (string-compare the integers — no float round-trips)
  const bound = parsed !== null
    && parsed.pair === feed.pair
    && parsed.mant === String(feed.mant)
    && parsed.expo === String(feed.expo)
    && parsed.ts === String(feed.signed_ts);
  const digest = blake2b256(asciiBytes(feed.message));
  const results = [];
  let valid = 0;
  for (let i = 0; i < signers.length; i++) {
    const pk = hexToBytes(String(signers[i]));
    const sig = hexToBytes(String(sigs[i] ?? ''));
    const ok = pk !== null && sig !== null && pk.length === 32 && sig.length === 64 && bip340Verify(pk, digest, sig);
    results.push({ signer: String(signers[i]), ok });
    if (ok) valid++;
  }
  const ok = bound && threshold > 0 && valid >= threshold;
  const out = { ok, valid, threshold, bound, parsed, results };
  if (parsed === null) out.error = 'unparsable signed message (want kaspulse/v2|PAIR|mant|expo|ts|round)';
  else if (!bound) out.error = 'signed message fields do not match the JSON fields (pair/mant/expo/signed_ts)';
  else if (threshold === 0) out.error = 'missing threshold';
  else if (valid < threshold) out.error = `only ${valid} of ${threshold} required signatures verify`;
  return out;
}

// ── mandatory self-test: refuse to verify anything with a broken core ───────
function runCoreSelfTest() {
  try {
    const abc = bytesToHex(blake2b256(asciiBytes('abc')));
    if (abc !== 'bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319') {
      return { ok: false, detail: 'blake2b-256("abc") known-answer mismatch' };
    }
    // BIP340 official test vector 0
    const pk = hexToBytes('f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9');
    const msg = new Uint8Array(32);
    const sig = hexToBytes('e907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca821525f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0');
    if (!bip340Verify(pk, msg, sig)) return { ok: false, detail: 'BIP340 test vector 0 did not verify' };
    const bad = sig.slice();
    bad[63] ^= 0x01;
    if (bip340Verify(pk, msg, bad)) return { ok: false, detail: 'corrupted BIP340 signature verified (core is broken)' };
    return { ok: true, detail: 'BIP340 vector 0 ✓, corrupted-copy rejected ✓, blake2b-256("abc") ✓' };
  } catch (e) {
    return { ok: false, detail: 'self-test threw: ' + ((e && e.message) || e) };
  }
}
const CORE_SELFTEST = runCoreSelfTest();
function selfTest() { return { ok: CORE_SELFTEST.ok, detail: CORE_SELFTEST.detail }; }
// CRYPTO-CORE-END
const st = selfTest();
if (!st.ok) console.error('kaspulse: verifier self-test failed — verify disabled:', st.detail);
window.kaspulseVerify = {
  ready: st.ok,
  selfTest,
  verifyFeed(feed) {
    if (!st.ok) {
      return { ok: false, valid: 0, threshold: 0, bound: false, parsed: null, results: [], error: 'verifier self-test failed: ' + st.detail };
    }
    try {
      return verifyFeedCore(feed);
    } catch (e) {
      return { ok: false, valid: 0, threshold: 0, bound: false, parsed: null, results: [], error: String((e && e.message) || e) };
    }
  },
};
})();
