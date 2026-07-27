import React from 'react';
import {AbsoluteFill, useCurrentFrame, useVideoConfig} from 'remotion';
import {T} from '../theme';
import {fade, glide, pop, countTo} from '../lib/anim';
import {Screen, Label, TokenChip, Check, Cross} from '../lib/ui';
import {DATA} from '../data';

/* Scene3 — The KRC-20 wall (240f · money shot #1, the flex).
   "PRICED FROM THE POOLS THEMSELVES" prints top; the hero line counts
   0→58 KRC-20 tokens in teal; a 6-col wall of violet-dotted TokenChips
   cascades in (pop staggered); then a competitor stamp row lands —
   Chainlink ✗ · Pyth ✗ · QUEX ✗ · kaspulse ✓ (kaspulse pops teal + glows).
   A dim micro-line seals it and the frame holds. */

// timing (local frames @60fps)
const LABEL_IN = 6;
const COUNT_FROM = 10;
const COUNT_TO = 50;
const CHIP0 = 40; // first chip pops
const CHIP_STEP = 4; // per-chip stagger
const STAMP = 152; // competitor row lands
const STAMP_STEP = 11; // gap between competitor stamps (5 of them — keep MICRO where it was)
// micro-line follows once the last competitor stamp has landed
const MICRO = STAMP + (DATA.competitors.length - 1) * STAMP_STEP + 14;

const COLS = 6;

export const Scene3: React.FC = () => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();

  const count = Math.round(countTo(frame, COUNT_FROM, COUNT_TO, DATA.krcCount));

  return (
    <Screen>
      <AbsoluteFill
        style={{
          alignItems: 'center',
          justifyContent: 'center',
          flexDirection: 'column',
          padding: T.margin,
          gap: 40,
        }}
      >
        {/* header label */}
        <Label
          style={{
            opacity: fade(frame, LABEL_IN, LABEL_IN + 12),
            transform: `translateY(${glide(frame, [LABEL_IN, LABEL_IN + 12], [-10, 0])}px)`,
            letterSpacing: 6,
            textAlign: 'center',
          }}
        >
          priced from the pools themselves
        </Label>

        {/* hero count line */}
        <div
          style={{
            display: 'flex',
            alignItems: 'baseline',
            gap: 24,
            opacity: fade(frame, COUNT_FROM, COUNT_FROM + 12),
            fontFamily: T.sans,
          }}
        >
          <span
            style={{
              fontSize: 120,
              fontWeight: 800,
              lineHeight: 1,
              letterSpacing: -3,
              color: T.teal,
              textShadow: T.tealGlow,
              fontVariantNumeric: 'tabular-nums',
            }}
          >
            {count}
          </span>
          <span
            style={{
              fontSize: 120,
              fontWeight: 800,
              lineHeight: 1,
              letterSpacing: -3,
              color: T.ink,
            }}
          >
            KRC-20 tokens
          </span>
        </div>

        {/* the wall of token chips */}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: `repeat(${COLS}, auto)`,
            gap: 18,
            justifyContent: 'center',
            maxWidth: 1500,
          }}
        >
          {DATA.krcTokens.map((name, i) => {
            const at = CHIP0 + i * CHIP_STEP;
            const p = pop(frame, at, fps);
            return (
              <div
                key={name}
                style={{
                  opacity: fade(frame, at, at + 8),
                  transform: `translateY(${(1 - p) * 12}px)`,
                }}
              >
                <TokenChip name={name} scale={0.94 + p * 0.06} />
              </div>
            );
          })}
        </div>

        {/* competitor stamp row */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 40,
            marginTop: 6,
          }}
        >
          {DATA.competitors.map((c, i) => {
            const at = STAMP + i * STAMP_STEP;
            const p = pop(frame, at, fps, c.covers ? 9 : 13);
            return (
              <React.Fragment key={c.name}>
                {i > 0 && (
                  <span
                    style={{
                      color: T.inkFaint,
                      fontSize: 30,
                      opacity: fade(frame, at, at + 8),
                    }}
                  >
                    ·
                  </span>
                )}
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 12,
                    opacity: fade(frame, at, at + 8),
                    transform: `translateY(${(1 - p) * 8}px) scale(${0.94 + p * 0.06})`,
                  }}
                >
                  {c.covers ? <Check lit={p} size={32} /> : <Cross size={30} />}
                  <span
                    style={{
                      fontFamily: T.sans,
                      fontSize: 34,
                      fontWeight: c.covers ? 800 : 600,
                      color: c.covers ? T.teal : T.inkDim,
                      textShadow: c.covers ? T.tealGlow : 'none',
                    }}
                  >
                    {c.name}
                  </span>
                </div>
              </React.Fragment>
            );
          })}
        </div>

        {/* dim micro-line */}
        <div
          style={{
            fontFamily: T.mono,
            fontSize: 24,
            color: T.inkFaint,
            letterSpacing: 1,
            opacity: fade(frame, MICRO, MICRO + 14),
          }}
        >
          the only oracle that prices Kaspa's own tokens.
          <div style={{marginTop: 10, fontSize: 20, color: T.inkFaint, opacity: 0.75}}>
            {DATA.competitorNote}
          </div>
        </div>
      </AbsoluteFill>
    </Screen>
  );
};
