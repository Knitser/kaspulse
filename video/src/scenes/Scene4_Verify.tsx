import React from 'react';
import {AbsoluteFill, useCurrentFrame, useVideoConfig} from 'remotion';
import {T} from '../theme';
import {fade, pop, typed, blink} from '../lib/anim';
import {Screen, Label, Panel, Check, SigRow, Cursor} from '../lib/ui';
import {DATA} from '../data';

/* Scene4 — Verify (240f). The trust moment. Two Inter lines slam in:
   "Don't trust this feed." → "Check the math." (teal). The signed message
   types across in mono inside a Panel, then the 5 committee signers stack as
   SigRows and their checks light one-by-one. Finally a teal banner pops —
   "3-of-5 verified — in your browser." with a dim "no trust required." This is
   the screenshot frame; it resolves and holds. */

const LINE1 = 6; // "Don't trust this feed."
const LINE2 = 30; // "Check the math."
const MSG_IN = 50; // signed message starts typing
const MSG_END = 95; // typing done
const SIG0 = 104; // first signer check lights
const SIG_STEP = 14; // stagger between signer checks
const BANNER = 200; // verified banner pops

export const Scene4: React.FC = () => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();

  const l1 = pop(frame, LINE1, fps, 11);
  const l2 = pop(frame, LINE2, fps, 11);

  const msg = typed(frame, MSG_IN, fps, DATA.signedMessage, 70);
  const typing = frame >= MSG_IN && msg.length < DATA.signedMessage.length;
  const cursorOn = typing ? 1 : blink(frame);

  const banner = pop(frame, BANNER, fps, 10);
  const bannerGlow = fade(frame, BANNER, BANNER + 12);

  return (
    <Screen>
      <AbsoluteFill
        style={{
          alignItems: 'center',
          justifyContent: 'center',
          padding: T.margin,
        }}
      >
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            gap: 30,
            opacity: fade(frame, 0, 12),
          }}
        >
          {/* the two slam lines */}
          <div style={{textAlign: 'center', lineHeight: 1.05}}>
            <div
              style={{
                fontFamily: T.sans,
                fontSize: 68,
                fontWeight: 800,
                letterSpacing: -1.5,
                color: T.ink,
                opacity: fade(frame, LINE1, LINE1 + 6),
                transform: `translateY(${(1 - l1) * 16}px) scale(${0.94 + l1 * 0.06})`,
              }}
            >
              Don&rsquo;t trust this feed.
            </div>
            <div
              style={{
                fontFamily: T.sans,
                fontSize: 84,
                fontWeight: 800,
                letterSpacing: -2,
                color: T.teal,
                textShadow: T.tealGlow,
                marginTop: 8,
                opacity: fade(frame, LINE2, LINE2 + 6),
                transform: `translateY(${(1 - l2) * 18}px) scale(${0.94 + l2 * 0.06})`,
              }}
            >
              Check the math.
            </div>
          </div>

          {/* the signed message types in a panel */}
          <Panel
            style={{
              padding: '22px 30px',
              minWidth: 900,
              opacity: fade(frame, MSG_IN - 8, MSG_IN + 4),
            }}
          >
            <Label style={{fontSize: 18, letterSpacing: 4, marginBottom: 12}}>
              signed message
            </Label>
            <div
              style={{
                fontFamily: T.mono,
                fontSize: 30,
                fontWeight: 500,
                color: T.ink,
                whiteSpace: 'pre',
                minHeight: 40,
              }}
            >
              {msg}
              <Cursor on={cursorOn > 0.5} height={30} />
            </div>
          </Panel>

          {/* committee signers — checks light one-by-one */}
          <div style={{display: 'flex', flexDirection: 'column', gap: 12}}>
            {DATA.signers.map((pk, i) => {
              const at = SIG0 + i * SIG_STEP;
              const lit = fade(frame, at, at + 14);
              const rowIn = fade(frame, at - 10, at - 2);
              return (
                <div
                  key={pk}
                  style={{
                    opacity: rowIn,
                    transform: `translateY(${(1 - rowIn) * 10}px)`,
                  }}
                >
                  <SigRow i={i + 1} pk={pk} lit={lit} />
                </div>
              );
            })}
          </div>

          {/* the verified banner — the screenshot frame */}
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              gap: 8,
              marginTop: 6,
              opacity: fade(frame, BANNER, BANNER + 6),
              transform: `translateY(${(1 - banner) * 14}px) scale(${0.92 + banner * 0.08})`,
            }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 16,
                padding: '18px 34px',
                background: `${T.teal}12`,
                border: `1px solid ${T.tealDim}88`,
                borderRadius: 14,
                boxShadow: `0 0 ${40 * bannerGlow}px ${T.teal}44`,
              }}
            >
              <Check lit={banner} size={38} />
              <span
                style={{
                  fontFamily: T.sans,
                  fontSize: 42,
                  fontWeight: 800,
                  letterSpacing: -1,
                  color: T.teal,
                  textShadow: T.tealGlow,
                }}
              >
                {DATA.threshold}-of-5 verified — in your browser.
              </span>
            </div>
            <Label style={{fontSize: 22, letterSpacing: 4, color: T.inkFaint}}>
              no trust required
            </Label>
          </div>
        </div>
      </AbsoluteFill>
    </Screen>
  );
};
