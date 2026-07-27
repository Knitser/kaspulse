import React from 'react';
import {AbsoluteFill, useCurrentFrame, useVideoConfig} from 'remotion';
import {T} from '../theme';
import {fade, glide, blink, typed} from '../lib/anim';
import {Screen, PulseSigil, Cursor} from '../lib/ui';

/* Scene1 — ColdOpen (180f). The problem. A single teal pulse-sigil DRAWS
   across center as a heartbeat, then three lines type in one at a time: two
   mono setup statements, then the Inter "turn" — the chain can't see a price
   (price in teal). Beneath, a dim covenant fragment stalls on a blinking red
   ??? where a number should be. Tension hold to the cut. */

// scene copy (narrative, not oracle data — safe to keep local)
const LINE1 = 'Kaspa moves value in a second.';
const LINE2 = 'Its coins can carry rules now.';
const TURN = "But the chain can't see a price.";

// timing (local frames @60fps)
const SIG_END = 50;
const T1 = 12; // line 1 begins typing
const T2 = 60; // line 2 begins typing
const T3 = 110; // the turn begins typing
const COV = 140; // covenant fragment fades in
const CPS = 38;

export const Scene1: React.FC = () => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();

  // sigil heartbeat draw, fading in over the first ~12 frames
  const draw = fade(frame, 0, SIG_END);
  const sigOpacity = fade(frame, 0, 12);

  // mono setup lines
  const str1 = typed(frame, T1, fps, LINE1, CPS);
  const str2 = typed(frame, T2, fps, LINE2, CPS);

  // the turn — typed, with the word "price" in teal
  const turnStr = typed(frame, T3, fps, TURN, CPS);
  const n = turnStr.length;
  const ps = TURN.indexOf('price');
  const pe = ps + 'price'.length;

  // one blinking cursor, riding whichever line is currently live
  const beat = blink(frame, 30) === 1;
  const cur1 = frame >= T1 && frame < T2 && beat;
  const cur2 = frame >= T2 && frame < T3 && beat;
  const cur3 = frame >= T3 && beat;

  // covenant fragment reveal + its blinking red ??? placeholder
  const covIn = fade(frame, COV, COV + 14);
  const covRise = glide(frame, [COV, COV + 16], [10, 0]);
  const holeGlow = 0.35 + 0.65 * blink(frame, 34);

  return (
    <Screen>
      <AbsoluteFill style={{alignItems: 'center', justifyContent: 'center'}}>
        <div
          style={{
            width: 1160,
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
          }}
        >
          {/* the heartbeat sigil */}
          <div style={{opacity: sigOpacity, marginBottom: 64}}>
            <PulseSigil size={280} draw={draw} />
          </div>

          {/* left-aligned type block so the cursor trails naturally */}
          <div
            style={{
              width: '100%',
              display: 'flex',
              flexDirection: 'column',
              gap: 22,
              textAlign: 'left',
            }}
          >
            {/* mono setup line 1 */}
            <div
              style={{
                minHeight: 52,
                fontFamily: T.mono,
                fontSize: 40,
                fontWeight: 500,
                color: T.ink,
                letterSpacing: 0.5,
              }}
            >
              {str1}
              <Cursor on={cur1} height={38} />
            </div>

            {/* mono setup line 2 */}
            <div
              style={{
                minHeight: 52,
                fontFamily: T.mono,
                fontSize: 40,
                fontWeight: 500,
                color: T.ink,
                letterSpacing: 0.5,
              }}
            >
              {str2}
              <Cursor on={cur2} height={38} />
            </div>

            {/* the turn — Inter, larger, "price" in teal */}
            <div
              style={{
                minHeight: 108,
                marginTop: 14,
                fontFamily: T.sans,
                fontSize: 86,
                fontWeight: 800,
                lineHeight: 1.05,
                letterSpacing: -2,
                color: T.ink,
              }}
            >
              <span>{TURN.slice(0, Math.min(n, ps))}</span>
              <span style={{color: T.teal, textShadow: T.tealGlow}}>
                {TURN.slice(ps, Math.min(n, pe))}
              </span>
              <span>{TURN.slice(pe, n)}</span>
              <Cursor on={cur3} height={72} />
            </div>

            {/* dim covenant fragment stalling on a blinking red ??? */}
            <div
              style={{
                marginTop: 30,
                opacity: covIn,
                transform: `translateY(${covRise}px)`,
                fontFamily: T.mono,
                fontSize: 32,
                letterSpacing: 1,
                color: T.inkFaint,
              }}
            >
              <span>{'if price ≥ '}</span>
              <span
                style={{
                  color: T.red,
                  fontWeight: 700,
                  opacity: holeGlow,
                  textShadow: `0 0 14px ${T.red}88`,
                }}
              >
                ???
              </span>
              <span>{' release'}</span>
            </div>
          </div>
        </div>
      </AbsoluteFill>
    </Screen>
  );
};
