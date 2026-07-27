/* Real numbers/strings from the live oracle (pulse.kascov.io) so the video
   never invents data. Update here, never hard-code in scenes.
   Last re-captured 2026-07-27, after `round` became a wall-clock slot — a
   viewer who curls /v1/feed must see the same SHAPE of round they see typed
   on screen. Nothing in this file may be invented: no txids, no rounds, no
   signer fragments. If you can't capture it, cut it from the scene. */

export const DATA = {
  url: 'pulse.kascov.io',
  wordmark: 'kaspulse',
  tagline: 'Every price Kaspa needs — signed & verifiable.',

  // headline majors (sub-second, CEX WebSocket median)
  majors: [
    {pair: 'KAS/USD', price: '$0.02919', sources: 4},
    {pair: 'BTC/USD', price: '$65,122', sources: 6},
    {pair: 'ETH/USD', price: '$1,956.50', sources: 6},
  ],

  // the flex: 58 KRC-20 tokens priced straight from Kasplex/Igra DEX pools
  krcCount: 58,
  krcTokens: [
    'NACHO', 'yoni', 'KDOG', 'PEPE', 'BONKEY', 'KREP', 'PLUTO', 'DOENER',
    'KANDA', 'KOOK', 'GAMER', 'AXEL', 'KEIRO', 'FIRST', 'SOMPG', 'KAST',
    'puppy', 'BMT', 'NICK', 'MARK', 'CYPUV', 'SPHERE', 'PICK', 'KOMA',
  ],

  // competitor coverage of Kaspa-native assets (all verified: none price KRC-20).
  // Kaskad COB is IN this list on purpose: it is the live Igra-mainnet oracle
  // (Sherlock-audited, KEF-funded, since May 2026), it is the strongest thing
  // anyone can reply with, and it prices zero KRC-20 — so volunteering it makes
  // the row stronger, not weaker. Leaving it out is one link from a refutation.
  competitors: [
    {name: 'Chainlink', covers: false},
    {name: 'Pyth', covers: false},
    {name: 'QUEX', covers: false},
    {name: 'Kaskad COB', covers: false},
    {name: 'kaspulse', covers: true},
  ],
  competitorNote: 'Kaskad COB is live on Igra mainnet for majors — use it if you need an audited KAS mark today.',

  // the trust moment — a real signed message + the 5 committee signer prefixes
  // captured 2026-07-27 from a running oracle. `round` is a wall-clock slot
  // (now_ms / 400ms), which is why it is ~4.46e9 and not a small counter.
  signedMessage: 'kaspulse/v2|KAS/USD|292000000|-10|1785159238|4462898095',
  threshold: 3,
  // first 8 and LAST 8 hex chars of each committee x-only pubkey, as served by
  // /v1/committee — check them against the live endpoint, they must match
  signers: [
    '0dd71bf2…086b8f4a',
    'd6ce84b1…43bf3bd2',
    '40ffebd6…adc5d514',
    '7d4e1759…6cc48864',
    '458220ba…066fdd0b',
  ],

  // the on-chain use case (proven on testnet-10)
  strike: '$0.0300',
  settlePrice: '$0.0301',
  latency: '~1.4s',
  // NOT "settled by kaspulse": the hosted committee's covenant domain was
  // withdrawn 2026-07-27, so the only committee that can sign a price gate is
  // one the consumer runs. Say that, or the video contradicts INTEGRATION.md.
  settleCaption: 'settled @ $0.0301 · 3-of-5 committee you run ✓',

  // honest status line (the repo's cardinal rule)
  honest: 'on-chain consumers ran on testnet-10 · the hosted covenant signature is withdrawn pending a bound preimage · mainnet next',
} as const;
