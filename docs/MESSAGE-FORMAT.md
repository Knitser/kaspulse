# kaspulse message format — v2 (normative)

*This is the interoperability spec. A third party should be able to write a
verifier from this document alone — no other source required. The reference
implementations (Rust `sdk/`, `clients/py/kaspulse.py`, `clients/js/kaspulse.mjs`)
all implement exactly what is written here.*

Version: **v2** (the literal prefix in every signed message). Any change to the
grammar, hash, or signature scheme bumps the version string; verifiers MUST
reject prefixes they don't know.

---

## 1. The signed message

Each attestation signs one ASCII string:

```
kaspulse/v2|PAIR|mant|expo|ts|round
```

Six fields joined by `|` (0x7C). No whitespace, no trailing separator, ASCII
only. Example (a real one — see the test vector in §9):

```
kaspulse/v2|KAS/USD|824000000|-10|1784380800|4242
```

| field | type | encoding | constraints |
|---|---|---|---|
| prefix | literal | `kaspulse/v2` | exact match, else reject |
| `PAIR` | string | uppercase, charset `A-Z 0-9 /` | e.g. `KAS/USD`, `NACHO/USD`. The `/` inside PAIR is unambiguous because `\|` is the field separator and PAIR never contains `\|` |
| `mant` | u64 | decimal, no sign, no leading zeros | normalized to 9 significant digits: `100000000 ≤ mant ≤ 999999999` for any positive price; `0` only for a zero/invalid price (which no consumer should accept) |
| `expo` | i32 | decimal, `-` sign iff negative, no `+`, no leading zeros | typically negative (e.g. `-10` for KAS at ~$0.08) |
| `ts` | u64 | decimal | unix seconds when this attestation was signed |
| `round` | u64 | decimal | oracle round counter at signing time |

The price is **`mant × 10^expo`** — exact at any magnitude. §6 gives the
normalization algorithm and why it exists.

## 2. The digest

```
digest = BLAKE2b(message_ascii_bytes)   — unkeyed, 32-byte output
```

Plain BLAKE2b with `digest_size = 32` (a.k.a. blake2b-256). No key, no salt,
no personalization, no domain tag outside the message's own `kaspulse/v2`
prefix. In Python: `hashlib.blake2b(msg, digest_size=32)`. In Rust:
`blake2b_simd::Params::new().hash_length(32).hash(msg)`.

Known answer for implementation sanity: `blake2b-256("abc")` =
`bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319`.

## 3. The signature

Each committee node signs the digest with **BIP340 Schnorr over secp256k1**,
where the 32-byte BIP340 message `m` is the blake2b digest itself:

```
sig_i = BIP340_Sign(seckey_i, m = digest)
valid = BIP340_Verify(pubkey_i, m = digest, sig_i)
```

**Explicitly: the digest is NOT hashed again outside BIP340's own tagged
hashing.** BIP340 internally computes
`e = int(tagged_hash("BIP0340/challenge", r ‖ P ‖ m)) mod n` — that is the only
further hashing. If your Schnorr library takes a "message" and hashes it with
SHA-256 first, do not use that path; pass the 32-byte digest as `m` directly.

Encodings:

- **`signers[i]`** — 32-byte **x-only** public key, lowercase hex (64 chars).
- **`signatures[i]`** — 64-byte BIP340 signature (`r ‖ s`), lowercase hex
  (128 chars).
- Verifiers SHOULD accept hex case-insensitively; kaspulse emits lowercase.

## 4. Committee, threshold, index pairing

The feed carries `signers` (n = `num_nodes` = 5 entries), `signatures`
(5 entries), and `threshold` (= 3). The arrays are **index-paired**:
`signatures[i]` is node `signers[i]`'s signature over the digest. There is no
subset selection or reordering — verify position by position.

```
VALID feed :=  |{ i : BIP340_Verify(signers[i], digest, signatures[i]) }| ≥ threshold
               AND the field-binding check of §5 passes
```

A verifier SHOULD report per-node results (which indexes verified), not just
the boolean — that is what the site's verify button and both clients do.

## 5. Field binding (REQUIRED)

A verifier **MUST** parse the message string and check that its fields equal
the JSON fields it is about to use:

```
message.PAIR == feed.pair
message.mant == feed.mant        (compare as strings or exact integers)
message.expo == feed.expo
message.ts   == feed.signed_ts
```

**Why this is not optional:** the signatures cover the *message string*, not
the JSON. Without this check, a compromised or buggy server could serve valid
signatures over one price next to JSON fields claiming another — every
signature verifies, and you still consume an unsigned number. Binding the
fields closes that gap; it is required by every kaspulse verifier (Rust SDK,
Python, JS, browser).

`message.round == feed.signed_round` is the same check for the round and
SHOULD also be enforced. Note the binding is against `signed_ts` /
`signed_round`, **not** the envelope's `timestamp` / `round` — see §7.

## 6. mant/expo — the 9-significant-digit normalization

The signed price is a mantissa/exponent pair, computed from the median `p`
(f64) as (from `src/main.rs`, `mant_expo()`):

```
if p <= 0 or p not finite:  (mant, expo) = (0, 0)
expo = floor(log10(p)) - 8
mant = round(p / 10^expo)
if mant >= 1_000_000_000:   # rounding carried into a 10th digit
    mant /= 10; expo += 1
```

Result: `mant` always has exactly 9 significant digits and
`p ≈ mant × 10^expo` to 9 significant digits at **any** magnitude.

**Why not just `price_e8`?** A fixed 8-decimal integer quantizes tiny prices
to zero: measured live, a $3e-9 KRC-20 token signed `price_e8 = 0` (100%
error), and other sub-1e-7 tokens signed with 3–27% error. The feed still
carries `price_e8` as an *informational* field; the **signed** number — the
only one a consumer should trust — is `mant × 10^expo`.

To compare against a strike, bring both to a common exponent using integer
arithmetic; do not round-trip through floats on-chain.

## 7. Timing semantics — sign on change, 5-second heartbeat

Prices are signed **when they change**, plus a heartbeat re-sign of unchanged
prices at most every **5 s**. Consequently a feed carries two clocks:

- **`signed_ts` / `signed_round`** — belong to the **signature**: when the
  attestation you hold was produced. This is what field binding checks and
  what freshness checks must use.
- **`timestamp` / `round`** (envelope level) — belong to the **serve tick**
  (~400 ms cadence): when the JSON you fetched was assembled.

A fresh envelope can legitimately carry an attestation up to ~5 s old (price
unchanged, heartbeat not yet due). Consumers enforce staleness against
`signed_ts` (e.g. the SDK's `checked_value_fresh(max_age)`).

## 8. On-chain encodings (bond record, price_bytes)

Two fixed binary encodings are used by the covenant tooling. The hosted
committee dual-signs:

1. the §1 *message string* (off-chain clients / SDK `Feed::verify`)
2. `blake2b(price_bytes)` and the 24-byte attestation record (on-chain
   covenants / SDK `Feed::verify_covenant`)

Each feed JSON includes a `covenant` object:

```json
"covenant": {
  "price_e8": 8240000,
  "price_bytes": "80bb7d",
  "signatures": ["…", "…", "…", "…", "…"],
  "record": "b84ad8389aa2ebb0…",
  "record_signatures": ["…", "…", "…", "…", "…"]
}
```

`signatures` are BIP340 over `blake2b-256(price_bytes)`; `record_signatures`
are BIP340 over `blake2b-256(record)`. Same `signers[]` order as the v2
message signatures. Pin the committee via `GET /v1/committee` and
`Feed::verify_with_committee` so keys are not learned only from the feed.

The `/guide.html` demo path can still use a local 3-key committee for a
fully offline walkthrough; production consumers should use the hosted
`covenant.signatures`.

### 8.1 The 24-byte attestation record (equivocation bond)

For the slashing bond, a node signs fixed-width records (from
`src/slash.rs` / the oracle build loop):

```
record (24 bytes) = blake2b-256(PAIR_ascii)[0..8]   # 8-byte pair id
                  ‖ round  as u64 big-endian        # 8 bytes
                  ‖ mant   as u64 big-endian        # 8 bytes
slot = record[0..16]  (pair id ‖ round)
sig  = BIP340_Sign(node_key, m = blake2b-256(record))
```

Two valid records with the **same slot** and a **different mant**, both signed
by one node key, are a proof of equivocation — the bond covenant verifies the
proof on L1 and releases the bond to whoever supplies it. Worked example
(PAIR `KAS/USD`, round 4242, mant 824000000):

```
blake2b-256("KAS/USD") = b84ad8389aa2ebb0a431e100443aac71ecb52569d5eb0bccb7546c2bbc9be61f
pair id  = b84ad8389aa2ebb0
record   = b84ad8389aa2ebb0 0000000000001092 00000000311d3e00
```

### 8.2 price_bytes — minimal script-number encoding

The covenant price is `price_e8` (i64) pushed as a **minimal
little-endian script number** (from `sdk/src/lib.rs`, `price_bytes()`):

```
0            → empty byte string
otherwise    → little-endian bytes of |price_e8|, minimal length;
               if the top byte has bit 0x80 set, append 0x00 (or 0x80 if
               negative); else if negative, set 0x80 on the top byte
```

Examples: `8240000` (= $0.0824 e8) → `80bb7d`; `128` → `8000`; `127` → `7f`.
Non-minimal encodings break the on-chain numeric comparison — encode exactly
this way. The demo nodes sign `BIP340(m = blake2b-256(price_bytes))`. The hosted
oracle publishes the same domain under `feed.covenant.signatures`.

## 9. Test vectors

### 9.1 Implementation sanity (embed these as self-tests)

Every kaspulse verifier runs these at load and refuses to run if they fail:

- **BIP340 official test vector 0** — pubkey
  `F9308A019258C31049344F85F89D5229B531C845836F99B08601F113BCE036F9`, message
  `0000…00` (32 zero bytes), signature
  `E907831F80848D1069A5371B402410364BDF1C5F8307B0084C55F1CE2DCA821525F66A4A85EA8B71E482A74F382D2CE5EBEEE8FDB2172F477DF4900D310536C0`
  → must verify (and a corrupted copy must not).
- **blake2b-256("abc")** → the known answer in §2.

### 9.2 The kaspulse end-to-end vector

One full example from message string to valid signature, generated with a
throwaway key. The signature uses BIP340 aux randomness of 32 zero bytes, so
the snippet below reproduces this output **byte-identically**:

```
secret key : 4fea87744110fb2fbf7d15b0b72f07fa3c47b20bb70b737d70b9f192df35e41f
signer     : 10a26a455a1abec4e1de900005b618f0dd0650db3ed842737d46f9d8793506af
message    : kaspulse/v2|KAS/USD|824000000|-10|1784380800|4242
digest     : 41f7fcbffcd7ccfeb8e0047a0b3e71e64c2d34a9923e63a118aeabb910efcb8c
signature  : 2102a2f6e3900436efb837f3cca8b64b50efb400feb7a4b873147c6679bc3ee8ef5575257329255b68f1b9d42b207f6bb4dcbb88c148160013e4e566d909316a
```

(The throwaway secret key is `blake2b-256("kaspulse test vector 1")` — derive
it, don't trust it. It secures nothing.)

A conforming verifier MUST accept this vector, and MUST reject it when any
single hex character of the message, digest, signature, or signer is changed.

Regenerate with this Rust program (same crates as the repo:
`secp256k1 = { version = "0.29", features = ["global-context", "rand-std"] }`,
`blake2b_simd = "1"`, `hex = "0.4"`):

```rust
use secp256k1::{Keypair, Message, SECP256K1};

fn main() {
    // throwaway secret key = blake2b-256("kaspulse test vector 1")
    let sk_bytes = blake2b_simd::Params::new()
        .hash_length(32)
        .hash(b"kaspulse test vector 1");
    let sk = secp256k1::SecretKey::from_slice(sk_bytes.as_bytes()).unwrap();
    let kp = Keypair::from_secret_key(SECP256K1, &sk);

    let message = "kaspulse/v2|KAS/USD|824000000|-10|1784380800|4242";
    let digest = blake2b_simd::Params::new()
        .hash_length(32)
        .hash(message.as_bytes());
    let msg = Message::from_digest_slice(digest.as_bytes()).unwrap();
    // aux_rand = 32 zero bytes → deterministic output (any BIP340-valid
    // signature over this digest is equally acceptable to a verifier)
    let sig = SECP256K1.sign_schnorr_with_aux_rand(&msg, &kp, &[0u8; 32]);

    println!("secret key : {}", hex::encode(sk_bytes.as_bytes()));
    println!("signer     : {}", hex::encode(kp.x_only_public_key().0.serialize()));
    println!("message    : {message}");
    println!("digest     : {}", hex::encode(digest.as_bytes()));
    println!("signature  : {}", hex::encode(sig.as_ref()));
    assert!(SECP256K1.verify_schnorr(&sig, &msg, &kp.x_only_public_key().0).is_ok());
    println!("self-check : signature verifies");
}
```

## 10. Verifier checklist

A conforming verifier, in order:

1. Parse the message; reject unless the prefix is exactly `kaspulse/v2` and
   there are exactly 6 fields.
2. **Bind the fields** (§5) — else report `bound = false` and do not use the
   price.
3. `digest = blake2b-256(message ASCII bytes)`.
4. For each `i`: `BIP340_Verify(signers[i], digest, signatures[i])`.
5. Accept iff valid count ≥ `threshold`.
6. Then honor the safety flags — `halted`, `thin`, `degraded`,
   `peg_ok == false` — and check freshness against `signed_ts`. Valid
   signatures over a halted or stale price are still a price you shouldn't use.
