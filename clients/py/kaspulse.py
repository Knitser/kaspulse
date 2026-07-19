#!/usr/bin/env python3
"""kaspulse.py — a tiny zero-dependency VERIFYING client for the kaspulse oracle.

Python 3.9+, stdlib only (hashlib + urllib). No pip installs, no keys.

    from kaspulse import Kaspulse
    k = Kaspulse('http://localhost:8080')
    feed = k.feed('KAS/USD')
    r = k.verify_feed(feed)      # {'ok': True, 'valid': 5, 'threshold': 3, 'bound': True, ...}
    px = k.checked_value(feed)   # verified + fresh + not halted, or ValueError

Honest scope: this verifies the committee's signatures (3-of-5 BIP340 Schnorr
over blake2b-256 of "kaspulse/v2|PAIR|mant|expo|ts|round"), the binding of the
signed message to the JSON fields, freshness and the safety flags. It does NOT
re-fetch the exchanges and recompute the median — that is `cargo run --bin verify`.

The crypto core self-tests at import (BIP340 official vector 0, a corrupted
copy that must FAIL, and blake2b-256(b"abc")); on any failure the import raises
RuntimeError rather than risk a fake verdict.

CLI:  python3 kaspulse.py verify KAS/USD [base]

Publishing to PyPI is a separate decision — this file is the whole client.
"""

import hashlib
import json
import re
import sys
import time
import urllib.error
import urllib.request

# ── hashes ──────────────────────────────────────────────────────────────────

def blake2b256(data):
    """Unkeyed blake2b, 32-byte digest — what the oracle hashes the message with."""
    return hashlib.blake2b(data, digest_size=32).digest()


def tagged_hash(tag, msg):
    """BIP340 tagged hash: sha256(sha256(tag) || sha256(tag) || msg)."""
    t = hashlib.sha256(tag.encode('ascii')).digest()
    return hashlib.sha256(t + t + msg).digest()


# ── BIP340 Schnorr verification over secp256k1 (verify-only, public data — no
#    side-channel concern; follows the BIP340 reference implementation) ───────

_P = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F
_N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
_G = (
    0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798,
    0x483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8,
)


def _lift_x(x):
    """The curve point with this x and EVEN y, or None (BIP340 lift_x)."""
    if x >= _P:
        return None
    c = (pow(x, 3, _P) + 7) % _P
    y = pow(c, (_P + 1) // 4, _P)
    if pow(y, 2, _P) != c:
        return None
    return (x, y if y % 2 == 0 else _P - y)


def _point_add(p1, p2):
    """Affine addition; None = point at infinity."""
    if p1 is None:
        return p2
    if p2 is None:
        return p1
    x1, y1 = p1
    x2, y2 = p2
    if x1 == x2 and (y1 + y2) % _P == 0:
        return None  # P + (−P)
    if p1 == p2:
        lam = (3 * x1 * x1 * pow(2 * y1, -1, _P)) % _P
    else:
        lam = ((y2 - y1) * pow(x2 - x1, -1, _P)) % _P
    x3 = (lam * lam - x1 - x2) % _P
    return (x3, (lam * (x1 - x3) - y1) % _P)


def _point_mul(pt, k):
    """Double-and-add scalar multiplication."""
    r = None
    while k > 0:
        if k & 1:
            r = _point_add(r, pt)
        pt = _point_add(pt, pt)
        k >>= 1
    return r


def bip340_verify(pubkey32, msg32, sig64):
    """Standard BIP340 verification. pubkey32 = x-only key, msg32 = the 32-byte
    message (for kaspulse: the blake2b-256 digest — NOT hashed again outside
    BIP340's own tagged hashing), sig64 = r||s. Returns bool, never raises."""
    if len(pubkey32) != 32 or len(msg32) != 32 or len(sig64) != 64:
        return False
    pt = _lift_x(int.from_bytes(pubkey32, 'big'))
    if pt is None:
        return False
    r = int.from_bytes(sig64[:32], 'big')
    s = int.from_bytes(sig64[32:], 'big')
    if r >= _P or s >= _N:
        return False
    e = int.from_bytes(tagged_hash('BIP0340/challenge', sig64[:32] + pubkey32 + msg32), 'big') % _N
    R = _point_add(_point_mul(_G, s), _point_mul(pt, _N - e))  # s·G − e·P
    if R is None or R[1] % 2 != 0 or R[0] != r:
        return False
    return True


# ── the kaspulse/v2 signed message and the feed verdict ─────────────────────

def parse_signed_message(message):
    """message = "kaspulse/v2|PAIR|mant|expo|ts|round" (ASCII, decimal ints,
    expo may be negative). Returns the five fields AS STRINGS (no float
    round-trips) or None if the shape is wrong."""
    if not isinstance(message, str):
        return None
    parts = message.split('|')
    if len(parts) != 6 or parts[0] != 'kaspulse/v2':
        return None
    pair, mant, expo, ts, rnd = parts[1:]
    if (not pair or not re.fullmatch(r'\d+', mant) or not re.fullmatch(r'-?\d+', expo)
            or not re.fullmatch(r'\d+', ts) or not re.fullmatch(r'\d+', rnd)):
        return None
    return {'pair': pair, 'mant': mant, 'expo': expo, 'ts': ts, 'round': rnd}


def _hex_bytes(s):
    """Lowercase-hex canonical, tolerant of uppercase; None on junk."""
    if not isinstance(s, str) or len(s) % 2 != 0:
        return None
    try:
        return bytes.fromhex(s)
    except ValueError:
        return None


def verify_feed(feed):
    """Verify one FeedObj (as parsed from /v1/feed/{PAIR}). Pure, no network.
    VALID := (count of BIP340-verifying signatures ≥ threshold) AND the signed
    message's PAIR/mant/expo/ts equal the JSON's pair/mant/expo/signed_ts.
    Returns {'ok', 'valid', 'threshold', 'bound', 'parsed', 'results'} (+'error')."""
    if not isinstance(feed, dict) or not isinstance(feed.get('message'), str):
        return {'ok': False, 'valid': 0, 'threshold': 0, 'bound': False,
                'parsed': None, 'results': [],
                'error': 'not a feed object (need message/signers/signatures)'}
    signers = feed.get('signers') if isinstance(feed.get('signers'), list) else []
    sigs = feed.get('signatures') if isinstance(feed.get('signatures'), list) else []
    threshold = feed.get('threshold') if isinstance(feed.get('threshold'), int) and feed.get('threshold') > 0 else 0
    parsed = parse_signed_message(feed['message'])
    # field binding: what was SIGNED must equal what the JSON claims
    # (string-compare the integers — no float round-trips)
    bound = (parsed is not None
             and parsed['pair'] == feed.get('pair')
             and parsed['mant'] == str(feed.get('mant'))
             and parsed['expo'] == str(feed.get('expo'))
             and parsed['ts'] == str(feed.get('signed_ts')))
    digest = blake2b256(feed['message'].encode('ascii', errors='replace'))
    results = []
    valid = 0
    for i, signer in enumerate(signers):
        pk = _hex_bytes(signer)
        sig = _hex_bytes(sigs[i]) if i < len(sigs) else None
        ok = (pk is not None and sig is not None and len(pk) == 32 and len(sig) == 64
              and bip340_verify(pk, digest, sig))
        results.append({'signer': str(signer), 'ok': ok})
        if ok:
            valid += 1
    ok = bound and threshold > 0 and valid >= threshold
    out = {'ok': ok, 'valid': valid, 'threshold': threshold, 'bound': bound,
           'parsed': parsed, 'results': results}
    if parsed is None:
        out['error'] = 'unparsable signed message (want kaspulse/v2|PAIR|mant|expo|ts|round)'
    elif not bound:
        out['error'] = 'signed message fields do not match the JSON fields (pair/mant/expo/signed_ts)'
    elif threshold == 0:
        out['error'] = 'missing threshold'
    elif valid < threshold:
        out['error'] = 'only %d of %d required signatures verify' % (valid, threshold)
    return out


def checked_value(feed, max_age_s=30):
    """The verified price (mant × 10^expo) — or ValueError with the reason it is
    unsafe: failed verification, halted, depegged, or older than max_age_s."""
    r = verify_feed(feed)
    if not r['ok']:
        raise ValueError('kaspulse: refusing value: ' + r.get('error', 'verification failed'))
    if feed.get('halted'):
        raise ValueError('kaspulse: refusing value: feed halted (circuit breaker)')
    if feed.get('peg_ok') is False:
        raise ValueError('kaspulse: refusing value: chain depegged (peg_ok=false)')
    age = time.time() - float(feed['signed_ts'])
    if age > max_age_s:
        raise ValueError('kaspulse: refusing value: signature is %.1f s old (max %s s)' % (age, max_age_s))
    return int(feed['mant']) * 10.0 ** int(feed['expo'])


# ── HTTP client ─────────────────────────────────────────────────────────────

class NoSuchFeed(Exception):
    def __init__(self, pair):
        super().__init__('kaspulse: no such feed: %s' % pair)
        self.pair = pair


# TODO(deploy): point this at the public *.run.app origin once the hosted
# oracle is deployed — until then the honest default is a locally-run oracle.
DEFAULT_BASE = 'http://localhost:8080'


class Kaspulse:
    def __init__(self, base_url=DEFAULT_BASE):
        self.base_url = str(base_url).rstrip('/')

    def _get(self, path):
        req = urllib.request.Request(self.base_url + path, headers={'Accept': 'application/json'})
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.load(resp)

    def feeds(self):
        """Light catalog: {round, timestamp, count, feeds:[{pair, price, ...}]}.
        This is the endpoint dashboards should poll."""
        return self._get('/v1/feeds')

    def feed(self, pair):
        """One full FeedObj (price, sources, signatures, history). pair like
        'KAS/USD' or 'KAS-USD', case-insensitive. Unknown pair → NoSuchFeed."""
        try:
            return self._get('/v1/feed/' + str(pair).replace('/', '-'))
        except urllib.error.HTTPError as e:
            if e.code == 404:
                raise NoSuchFeed(pair) from None
            raise

    def verify_feed(self, feed):
        """See module-level verify_feed()."""
        return verify_feed(feed)

    def checked_value(self, feed, max_age_s=30):
        """See module-level checked_value()."""
        return checked_value(feed, max_age_s)


# ── mandatory import-time self-test: refuse to run with a broken core ───────

def _self_test():
    abc = blake2b256(b'abc').hex()
    if abc != 'bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319':
        return 'blake2b-256(b"abc") known-answer mismatch'
    # BIP340 official test vector 0
    pk = bytes.fromhex('f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9')
    msg = bytes(32)
    sig = bytes.fromhex('e907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca821525f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0')
    if not bip340_verify(pk, msg, sig):
        return 'BIP340 test vector 0 did not verify'
    bad = bytearray(sig)
    bad[63] ^= 0x01
    if bip340_verify(pk, msg, bytes(bad)):
        return 'corrupted BIP340 signature verified (core is broken)'
    return None


_SELF_TEST_ERROR = _self_test()
if _SELF_TEST_ERROR is not None:
    raise RuntimeError('kaspulse verifier self-test failed: ' + _SELF_TEST_ERROR)


# ── CLI: python3 kaspulse.py verify KAS/USD [base] ──────────────────────────

def _main(argv):
    if len(argv) < 2 or argv[0] != 'verify':
        print('usage: python3 kaspulse.py verify KAS/USD [base]', file=sys.stderr)
        return 2
    pair = argv[1].replace('-', '/')
    base = argv[2] if len(argv) > 2 else DEFAULT_BASE
    k = Kaspulse(base)
    try:
        feed = k.feed(pair)
    except NoSuchFeed as e:
        print('✗ %s' % e, file=sys.stderr)
        return 1
    except (urllib.error.URLError, OSError) as e:
        print('✗ kaspulse: cannot reach %s: %s' % (base, e), file=sys.stderr)
        return 1
    r = verify_feed(feed)
    print('%s  %s  signed_round %s' % (feed.get('pair'), base, feed.get('signed_round', '?')))
    for i, node in enumerate(r['results']):
        print('  node %d  %s  %s' % (i, '✓' if node['ok'] else '✗', node['signer']))
    print('  bound=%s (signed message fields == JSON fields)' % str(r['bound']).lower())
    if r['ok']:
        px = int(feed['mant']) * 10.0 ** int(feed['expo'])
        print('✓ VALID — %d/%d signatures verify (threshold %d), price = %s'
              % (r['valid'], len(r['results']), r['threshold'], px))
        return 0
    print('✗ INVALID — %s' % r.get('error', 'verification failed'))
    return 1


if __name__ == '__main__':
    sys.exit(_main(sys.argv[1:]))
