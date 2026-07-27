import React from 'react';
import {AbsoluteFill, useCurrentFrame, useVideoConfig} from 'remotion';
import {T} from '../theme';
import {fade, glide, pop} from '../lib/anim';
import {Screen, Wordmark, FeedRow, Label} from '../lib/ui';
import {DATA} from '../data';

/* Scene2 — Enter kaspulse (240f). The pulse resolves into the brand: the
   wordmark pops in center-top, the tagline fades under it, then the live feed
   board assembles below — the 3 majors gliding up + fading in staggered
   (KAS/USD featured), each with its price and 3-of-5 ✓ badge. A dim
   "+ N more, live" label lands beneath. Holds on the full board. */

// timing (local frames @60fps)
const WORDMARK_IN = 8;
const TAGLINE_IN = 30;
const FEED0 = 60; // first feed row glides in
const FEED_STEP = 12; // stagger between rows
const MORE_IN = 150; // "+ N more, live" label

export const Scene2: React.FC = () => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();

  const wm = pop(frame, WORDMARK_IN, fps, 11); // 0→1 overshoot

  return (
    <Screen>
      <AbsoluteFill
        style={{
          alignItems: 'center',
          justifyContent: 'center',
          flexDirection: 'column',
          padding: T.margin,
          gap: 20,
        }}
      >
        {/* wordmark pops in */}
        <div
          style={{
            opacity: fade(frame, WORDMARK_IN, WORDMARK_IN + 8),
            transform: `translateY(${(1 - wm) * -14}px) scale(${0.9 + wm * 0.1})`,
          }}
        >
          <Wordmark size={110} />
        </div>

        {/* tagline fades under it */}
        <div
          style={{
            fontFamily: T.sans,
            fontSize: 40,
            fontWeight: 500,
            color: T.inkDim,
            letterSpacing: -0.3,
            opacity: fade(frame, TAGLINE_IN, TAGLINE_IN + 14),
            transform: `translateY(${glide(frame, [TAGLINE_IN, TAGLINE_IN + 14], [8, 0])}px)`,
            marginBottom: 26,
          }}
        >
          {DATA.tagline}
        </div>

        {/* the live feed board assembles, staggered */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'stretch',
            gap: 16,
          }}
        >
          {DATA.majors.map((m, i) => {
            const at = FEED0 + i * FEED_STEP;
            const p = fade(frame, at, at + 16);
            return (
              <div
                key={m.pair}
                style={{
                  transform: `translateY(${(1 - p) * 26}px)`,
                }}
              >
                <FeedRow
                  pair={m.pair}
                  price={m.price}
                  sources={m.sources}
                  featured={i === 0}
                  opacity={p}
                />
              </div>
            );
          })}
        </div>

        {/* "+ N more, live" nod to the full coverage */}
        <Label
          color={T.inkFaint}
          style={{
            marginTop: 24,
            letterSpacing: 4,
            opacity: fade(frame, MORE_IN, MORE_IN + 16) * 0.85,
            transform: `translateY(${glide(frame, [MORE_IN, MORE_IN + 16], [6, 0])}px)`,
          }}
        >
          + {DATA.krcCount} more, live
        </Label>
      </AbsoluteFill>
    </Screen>
  );
};
