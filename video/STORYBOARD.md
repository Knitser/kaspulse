# kaspulse launch video — frozen storyboard & scene contract

**Format:** 1920×1080, **60 fps**, **1320 frames total (22.0s)**. Clean "signed & verifiable" identity — teal signal, violet accent, gold strike, deep near-black. NOT thermal/jittery; precise, techy, confident.

**Concept:** the chain that could move value in a second but couldn't *see* a price — now can, verifiably. A teal pulse waveform is the through-line. Three money moments: the KRC-20 wall (the flex), the in-browser 3-of-5 verify (the trust), and the on-chain covenant payout (the use case).

## Global rules (every scene)
- Each scene is `export const SceneN: React.FC = () => {…}`, self-contained, full-frame, using `useCurrentFrame()` starting at **local 0** (Root wraps each in `<Sequence>`; do NOT add your own Sequence).
- Render inside `<Screen>` from `lib/ui.tsx`. Use `T` tokens from `theme.ts`, helpers from `lib/anim.ts` (`fade, glide, pop, countTo, blink, typed`), components from `lib/ui.tsx` (`Screen, Center, PulseSigil, Wordmark, Label, Panel, Rule, Check, Cross, FeedRow, TokenChip, SigRow, Cursor`), and strings/numbers from `data.ts` (`DATA`) — never hard-code data that lives in `data.ts`.
- `pop()` needs `fps` from `useVideoConfig()`.
- Motion: cubic-out eases; teal glow on reveals; monospace for all data/signatures/hex; Inter for display headlines. The ONLY gold is the strike line + hero settle price; teal is the brand/verified; violet is the KRC-20 accent; red only for ✗/halt.
- Type scale: hero display 96–160px/800 (Inter); data/mono 24–40px; labels via `<Label>` (24px tracked). ~96px margins.
- Each scene RESOLVES and HOLDS on its last ~15–20 frames (no mid-animation cut) so Root hard-cuts cleanly. Fade the scene's own content in over the first ~12 frames.

## Scenes (each an owned file in src/scenes/)

### Scene1 — ColdOpen · `src/scenes/Scene1_ColdOpen.tsx` · 180 frames — the problem
Deep black. A single teal `<PulseSigil>` stroke DRAWS across center-large (draw 0→1, f0–50) — a heartbeat. Mono lines type in (use `typed`), one at a time with a `<Cursor>`:
- f10: "Kaspa moves value in a second."
- f60: "Its coins can carry rules now."
- f110 (the turn, larger, Inter): "But the chain can't see a **price**." — 'price' in teal.
Beneath at ~f140 a dim covenant fragment: `if price ≥ ??? release` with the `???` blinking red where a number should be. Hold the tension.

### Scene2 — Enter kaspulse · `src/scenes/Scene2_Enter.tsx` · 240 frames — the solution
The pulse resolves: `<Wordmark size={110}>` pops in center-top (`pop`, f8). Tagline `DATA.tagline` fades under it (Inter, 40px, f30). Then a live feed board assembles below (f60+): the 3 `DATA.majors` as `<FeedRow>` (KAS/USD `featured`), each gliding up + fading in staggered (~12f apart), price + "3-of-5 ✓" badge. At ~f150 a dim `<Label>` under the board: "+ 58 more, live" (nod to what's coming). Hold on the full board.

### Scene3 — The KRC-20 wall · `src/scenes/Scene3_Krc.tsx` · 240 frames — money shot #1 (the flex)
`<Label>` top: "PRICED FROM THE POOLS THEMSELVES". A hero line (Inter, 120px/800): **`{DATA.krcCount}` KRC-20 tokens** with the number `countTo`-counting 0→58 (f10–50), teal. Then a WALL of `<TokenChip>` from `DATA.krcTokens` cascades in (grid, ~6 cols, each `pop` staggered ~4f, f40–140) — yoni, PEPE, BONKEY, NACHO… violet-dotted. At ~f150 a stamp row of `DATA.competitors`: Chainlink ✗ · Pyth ✗ · QUEX ✗ · Kaskad COB ✗ · **kaspulse ✓** (use `<Cross>` / `<Check>`, kaspulse pops teal + glows). Kaskad COB is in the row deliberately — it is the live Igra-mainnet oracle and omitting it is one link from a refutation; the ✗ is accurate because it prices no KRC-20. Micro-line under, dim: "the only oracle that prices Kaspa's own tokens." plus `DATA.competitorNote` ("use Kaskad COB if you need an audited KAS mark today"). Hold.

### Scene4 — Don't trust, verify · `src/scenes/Scene4_Verify.tsx` · 240 frames — the trust moment
Dark, minimal. Two lines slam in (Inter, `pop`): "Don't trust this feed." (f6) → "Check the math." (f30, teal). The signed message `DATA.signedMessage` types across in mono (f50–95, `typed`), in a `<Panel>`. Then the 5 `DATA.signers` as `<SigRow>` stack in, and their `<Check>` lights one-by-one (each `lit` driven by `fade` staggered ~14f from f100). At ~f200 a banner pops: "**{DATA.threshold}-of-5 verified — in your browser.**" (teal, glow) with a dim "no trust required." This is the screenshot frame — hold.

### Scene5 — On-chain use case · `src/scenes/Scene5_OnChain.tsx` · 240 frames — money shot #2 (the thesis)
`<Label>` top: "IT DOESN'T STOP AT AN API". A vertical price gauge/needle rises toward a glowing GOLD strike line labelled `DATA.strike` (needle glides up f10–90). The instant it crosses (~f95): a flash, and a covenant `<Panel>` shows two conditions checking: "verify sig" `<Check>` + "price ≥ strike" `<Check>` (pop f100/f115). Then a coin/token flies out (a `<PulseSigil>` or chip translating) → a payout, and a mono stamp lands (`pop`, f150): `DATA.settleCaption` + a latency line `tick → spendable UTXO · {DATA.latency}`. **No txid on screen** — a testnet hash is pruned within days, and an invented one is the cheapest possible refutation of a "verify it yourself" launch. Headline under (Inter, 80px): "The chain finally has eyes." Hold. (Honest micro-tag dim at the very bottom: `DATA.honest`.)

### Scene6 — EndCard · `src/scenes/Scene6_EndCard.tsx` · 180 frames — resolve
Calm. `<PulseSigil size={120}>` draws + `<Wordmark size={130}>` pops center (f8). Under it (Inter, 40px, ink): "the covenant-ready price oracle for Kaspa." Then the URL `DATA.url` in teal (36px mono, `pop` f60) with a subtle underline glow, then dim "verify it yourself →" (f90). Slow, confident hold to the end.

## Root
`src/Root.tsx` registers one `<Composition id="Launch" fps={60} width={1920} height={1080} durationInFrames={1320}>` sequencing Scene1(0,180) → Scene2(180,240) → Scene3(420,240) → Scene4(660,240) → Scene5(900,240) → Scene6(1140,180). Imports `./fonts`.
