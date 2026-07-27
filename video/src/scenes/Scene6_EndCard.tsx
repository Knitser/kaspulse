import React from 'react';
import {AbsoluteFill, useCurrentFrame, useVideoConfig} from 'remotion';
import {T} from '../theme';
import {fade, glide, pop} from '../lib/anim';
import {Screen, Center, PulseSigil, Wordmark} from '../lib/ui';
import {DATA} from '../data';

/* Scene6 — EndCard (180f). The film resolves: a large teal PulseSigil draws
   across center, the wordmark pops beside it, the positioning line fades in,
   then the URL pops in teal with an underline glow and a dim "verify it
   yourself →" invitation. Slow, confident hold to the end. */

// timing (local frames @60fps)
const SIGIL_DRAW_FROM = 8;
const SIGIL_DRAW_TO = 58;
const MARK_POP = 8;
const TAGLINE_IN = 40;
const URL_POP = 60;
const VERIFY_IN = 90;

export const Scene6: React.FC = () => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();

  const draw = glide(frame, [SIGIL_DRAW_FROM, SIGIL_DRAW_TO], [0, 1]);
  const markPop = pop(frame, MARK_POP, fps, 13);
  const urlPop = pop(frame, URL_POP, fps, 12);

  // gentle underline glow breathing under the URL, once it has landed
  const glowPhase = 0.5 + 0.5 * Math.sin((frame - URL_POP) * 0.09);
  const underlineGlow = urlPop * (0.35 + 0.35 * glowPhase);

  return (
    <Screen>
      <Center style={{gap: 40}}>
        {/* sigil + wordmark lockup */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 34,
            opacity: fade(frame, MARK_POP, MARK_POP + 12),
            transform: `translateY(${(1 - markPop) * 14}px) scale(${
              0.92 + markPop * 0.08
            })`,
          }}
        >
          <PulseSigil size={120} draw={draw} />
          <div style={{fontFamily: T.sans, fontSize: 130, fontWeight: 800, letterSpacing: -2, color: T.ink}}>
            kasp<span style={{color: T.teal, textShadow: T.tealGlow}}>u</span>lse
          </div>
        </div>

        {/* positioning line */}
        <div
          style={{
            fontFamily: T.sans,
            fontSize: 40,
            fontWeight: 500,
            color: T.ink,
            letterSpacing: -0.3,
            opacity: fade(frame, TAGLINE_IN, TAGLINE_IN + 16),
            transform: `translateY(${glide(frame, [TAGLINE_IN, TAGLINE_IN + 16], [8, 0])}px)`,
          }}
        >
          the covenant-ready price oracle for Kaspa.
        </div>

        {/* URL — the call to action */}
        <div
          style={{
            marginTop: 18,
            display: 'inline-flex',
            flexDirection: 'column',
            alignItems: 'center',
            gap: 8,
            opacity: fade(frame, URL_POP, URL_POP + 8),
            transform: `scale(${0.86 + urlPop * 0.14})`,
          }}
        >
          <span
            style={{
              fontFamily: T.mono,
              fontSize: 36,
              fontWeight: 600,
              color: T.teal,
              textShadow: T.tealGlow,
            }}
          >
            {DATA.url}
          </span>
          <div
            style={{
              width: '100%',
              height: 2,
              background: T.teal,
              borderRadius: 2,
              boxShadow: `0 0 ${10 + underlineGlow * 22}px rgba(73,234,203,${0.4 + underlineGlow * 0.5})`,
              transform: `scaleX(${urlPop})`,
            }}
          />
        </div>

        {/* verify invitation */}
        <div
          style={{
            fontFamily: T.mono,
            fontSize: 24,
            letterSpacing: 3,
            color: T.inkFaint,
            opacity: fade(frame, VERIFY_IN, VERIFY_IN + 18),
            transform: `translateY(${glide(frame, [VERIFY_IN, VERIFY_IN + 18], [6, 0])}px)`,
          }}
        >
          verify it yourself →
        </div>
      </Center>
    </Screen>
  );
};
