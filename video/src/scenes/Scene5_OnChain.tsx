import React from 'react';
import {AbsoluteFill, useCurrentFrame, useVideoConfig} from 'remotion';
import {T} from '../theme';
import {fade, glide, pop} from '../lib/anim';
import {Screen, Label, Panel, Check, PulseSigil} from '../lib/ui';
import {DATA} from '../data';

/* Scene5 — On-chain use case (240f) · money shot #2 / the thesis.
   "IT DOESN'T STOP AT AN API": a teal price needle rises up a vertical gauge
   toward a glowing GOLD strike line (DATA.strike). The instant it crosses
   (~f95) a teal flash fires and a covenant Panel checks its two conditions —
   "verify sig" then "price ≥ strike". A pulse-coin then flies from the gauge
   into a PAYOUT slot; a mono settle stamp lands (DATA.settleCaption +
   DATA.latency). NO TXID: testnet runs are pruned and the guide deleted its
   pinned txids for that reason, so a hash on screen would be either dead or
   invented — and an invented one is the cheapest possible refutation of a
   "verify it yourself" launch. Headline "The chain finally has eyes." resolves
   and holds, with the honest testnet tag dim at the very bottom. */

// timing (local frames @60fps)
const LABEL_IN = 6;
const NEEDLE_START = 10;
const NEEDLE_END = 90;
const CROSS = 95;
const CHECK1 = 100;
const CHECK2 = 115;
const COIN_START = 125;
const COIN_END = 150;
const STAMP = 150;
const HEADLINE = 166;
const HONEST = 190;

// stage geometry
const STAGE_W = 1500;
const STAGE_H = 480;
const TRACK_H = 420;
const BOTTOM_Y = STAGE_H - 30; // px from stage top to gauge base
const STRIKE_FRAC = 0.72; // strike height as fraction of the track
const FILL_MAX = 0.82; // needle settles just above the strike

export const Scene5: React.FC = () => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();

  // whole-scene fade-in
  const enter = fade(frame, 0, 12);

  // needle rise → crosses the gold strike ~f95
  const fillFrac = glide(frame, [NEEDLE_START, NEEDLE_END], [0, FILL_MAX]);
  const strikeY = BOTTOM_Y - STRIKE_FRAC * TRACK_H;
  const fillTopY = BOTTOM_Y - fillFrac * TRACK_H;

  // teal verify-flash on the crossing
  const flash =
    glide(frame, [CROSS, CROSS + 4], [0, 0.32]) -
    glide(frame, [CROSS + 4, CROSS + 16], [0, 0.32]);

  // covenant condition checks
  const lit1 = fade(frame, CHECK1, CHECK1 + 16);
  const lit2 = fade(frame, CHECK2, CHECK2 + 16);
  const covenantPop = pop(frame, CHECK1 - 8, fps, 13);

  // coin flight: from the needle top → the PAYOUT slot, with a small arc
  const coinP = glide(frame, [COIN_START, COIN_END], [0, 1]);
  const coinStart = {x: 185, y: 110};
  const coinEnd = {x: 1300, y: 210};
  const coinX = coinStart.x + (coinEnd.x - coinStart.x) * coinP;
  const coinY =
    coinStart.y +
    (coinEnd.y - coinStart.y) * coinP -
    90 * Math.sin(Math.PI * coinP);
  const coinOpacity = fade(frame, COIN_START, COIN_START + 6) - fade(frame, COIN_END, COIN_END + 8);
  const payoutLit = fade(frame, COIN_END - 2, COIN_END + 12);

  // settle stamp
  const stampPop = pop(frame, STAMP, fps, 10);
  // latency only — see the header note on why there is no txid here
  const settleMeta = `tick → spendable UTXO · ${DATA.latency}`;

  return (
    <Screen>
      {/* teal verify flash on the strike crossing */}
      <AbsoluteFill
        style={{
          background: T.teal,
          opacity: Math.max(0, flash),
          mixBlendMode: 'screen',
          pointerEvents: 'none',
        }}
      />

      <AbsoluteFill
        style={{
          alignItems: 'center',
          justifyContent: 'center',
          flexDirection: 'column',
          padding: T.margin,
          gap: 30,
          opacity: enter,
        }}
      >
        {/* header label */}
        <Label
          style={{
            opacity: fade(frame, LABEL_IN, LABEL_IN + 14),
            letterSpacing: 6,
            transform: `translateY(${glide(frame, [LABEL_IN, LABEL_IN + 14], [-8, 0])}px)`,
          }}
        >
          it doesn't stop at an api
        </Label>

        {/* THE STAGE: gauge · covenant panel · payout · flying coin */}
        <div style={{position: 'relative', width: STAGE_W, height: STAGE_H}}>
          {/* ── vertical price gauge ── */}
          <div style={{position: 'absolute', left: 0, top: 0, width: 260, height: STAGE_H}}>
            {/* track */}
            <div
              style={{
                position: 'absolute',
                left: 70,
                bottom: 30,
                width: 84,
                height: TRACK_H,
                background: T.panel,
                border: `1px solid ${T.panelEdge}`,
                borderRadius: 12,
                overflow: 'hidden',
              }}
            >
              {/* rising teal fill */}
              <div
                style={{
                  position: 'absolute',
                  left: 0,
                  bottom: 0,
                  width: '100%',
                  height: fillFrac * TRACK_H,
                  background: `linear-gradient(180deg, ${T.teal} 0%, ${T.tealDim} 100%)`,
                  opacity: 0.85,
                }}
              />
            </div>

            {/* glowing needle cap */}
            <div
              style={{
                position: 'absolute',
                left: 64,
                width: 96,
                height: 4,
                top: fillTopY - 2,
                background: T.teal,
                boxShadow: T.tealGlow,
                borderRadius: 2,
              }}
            />

            {/* gold strike line */}
            <div
              style={{
                position: 'absolute',
                left: 52,
                width: 120,
                top: strikeY,
                height: 0,
                borderTop: `2px dashed ${T.gold}`,
                boxShadow: `0 0 14px ${T.gold}88`,
              }}
            />
            <div
              style={{
                position: 'absolute',
                left: 182,
                top: strikeY - 15,
                fontFamily: T.mono,
                fontSize: 24,
                color: T.gold,
              }}
            >
              strike {DATA.strike}
            </div>

            {/* settle price rides the needle top once crossed */}
            <div
              style={{
                position: 'absolute',
                left: 182,
                top: fillTopY - 15,
                fontFamily: T.mono,
                fontSize: 22,
                fontWeight: 700,
                color: T.gold,
                textShadow: `0 0 14px ${T.gold}66`,
                opacity: fade(frame, CROSS, CROSS + 14),
              }}
            >
              {DATA.settlePrice}
            </div>
          </div>

          {/* ── covenant panel ── */}
          <Panel
            style={{
              position: 'absolute',
              left: 360,
              top: (STAGE_H - 240) / 2,
              width: 640,
              padding: '30px 40px',
              display: 'flex',
              flexDirection: 'column',
              gap: 24,
              opacity: fade(frame, CHECK1 - 12, CHECK1),
              transform: `scale(${0.96 + covenantPop * 0.04})`,
              borderColor: `${T.tealDim}66`,
            }}
          >
            <Label style={{letterSpacing: 4, color: T.inkDim}}>covenant</Label>
            {[
              {label: 'verify sig', lit: lit1},
              {label: 'price ≥ strike', lit: lit2},
            ].map((c) => (
              <div
                key={c.label}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                }}
              >
                <span
                  style={{
                    fontFamily: T.mono,
                    fontSize: 32,
                    color: c.lit > 0.5 ? T.ink : T.inkDim,
                  }}
                >
                  {c.label}
                </span>
                <Check lit={c.lit} size={34} />
              </div>
            ))}
          </Panel>

          {/* ── payout slot ── */}
          <Panel
            style={{
              position: 'absolute',
              left: 1160,
              top: (STAGE_H - 150) / 2,
              width: 280,
              height: 150,
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 12,
              opacity: fade(frame, CHECK1, CHECK1 + 14),
              borderColor: payoutLit > 0.5 ? `${T.tealDim}88` : T.panelEdge,
              boxShadow:
                payoutLit > 0.5
                  ? `0 0 40px ${T.teal}44, 0 40px 120px rgba(0,0,0,0.5)`
                  : '0 40px 120px rgba(0,0,0,0.5)',
            }}
          >
            <PulseSigil size={44} glow={payoutLit > 0.6} color={payoutLit > 0.6 ? T.teal : T.inkFaint} />
            <Label style={{letterSpacing: 5, color: payoutLit > 0.5 ? T.teal : T.inkFaint}}>
              payout
            </Label>
          </Panel>

          {/* ── flying pulse-coin ── */}
          <div
            style={{
              position: 'absolute',
              left: coinX,
              top: coinY,
              opacity: Math.max(0, coinOpacity),
              transform: 'translate(-50%, -50%)',
            }}
          >
            <PulseSigil size={56} color={T.gold} />
          </div>
        </div>

        {/* ── settle stamp ── */}
        <Panel
          style={{
            padding: '18px 34px',
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            gap: 8,
            opacity: fade(frame, STAMP, STAMP + 8),
            transform: `scale(${0.94 + stampPop * 0.06})`,
            borderColor: `${T.tealDim}66`,
          }}
        >
          <span style={{fontFamily: T.mono, fontSize: 30, color: T.ink, fontWeight: 600}}>
            {DATA.settleCaption}
          </span>
          <span style={{fontFamily: T.mono, fontSize: 22, color: T.inkFaint, letterSpacing: 1}}>
            {settleMeta}
          </span>
        </Panel>

        {/* ── resolving headline ── */}
        <div
          style={{
            fontFamily: T.sans,
            fontSize: 80,
            fontWeight: 800,
            letterSpacing: -1.5,
            color: T.ink,
            textAlign: 'center',
            opacity: fade(frame, HEADLINE, HEADLINE + 16),
            transform: `translateY(${glide(frame, [HEADLINE, HEADLINE + 16], [14, 0])}px)`,
          }}
        >
          The chain finally has eyes.
        </div>
      </AbsoluteFill>

      {/* honest testnet tag, dim at the very bottom */}
      <div
        style={{
          position: 'absolute',
          left: 0,
          right: 0,
          bottom: 44,
          textAlign: 'center',
          fontFamily: T.mono,
          fontSize: 22,
          color: T.inkFaint,
          letterSpacing: 1,
          opacity: fade(frame, HONEST, HONEST + 16),
        }}
      >
        {DATA.honest}
      </div>
    </Screen>
  );
};
