import React from 'react';
import {AbsoluteFill} from 'remotion';
import {T} from '../theme';

/* Shared chrome. Every scene renders inside <Screen> so the deep-space
   background, faint signal grid and vignette are identical everywhere — this
   is what makes the scenes cohere into one film. */
export const Screen: React.FC<{children: React.ReactNode; grid?: boolean}> = ({
  children,
  grid = true,
}) => (
  <AbsoluteFill
    style={{
      background: `radial-gradient(130% 100% at 50% -20%, ${T.bg2} 0%, ${T.bg} 62%)`,
      fontFamily: T.mono,
      color: T.ink,
      // JetBrains Mono ligatures turn `|-` etc. into box-drawing glyphs —
      // kill them so the signed message renders as literal pipes/dashes.
      fontFeatureSettings: '"liga" 0, "calt" 0',
      fontVariantLigatures: 'none',
    }}
  >
    {grid && (
      <AbsoluteFill
        style={{
          pointerEvents: 'none',
          backgroundImage: `linear-gradient(${T.panelEdge}22 1px, transparent 1px), linear-gradient(90deg, ${T.panelEdge}22 1px, transparent 1px)`,
          backgroundSize: '64px 64px',
          maskImage: 'radial-gradient(80% 70% at 50% 45%, #000 30%, transparent 100%)',
          WebkitMaskImage: 'radial-gradient(80% 70% at 50% 45%, #000 30%, transparent 100%)',
          opacity: 0.5,
        }}
      />
    )}
    {children}
    {/* vignette */}
    <AbsoluteFill
      style={{pointerEvents: 'none', boxShadow: 'inset 0 0 320px rgba(0,0,0,0.7)'}}
    />
  </AbsoluteFill>
);

/* Center helper. */
export const Center: React.FC<{children: React.ReactNode; style?: React.CSSProperties}> = ({
  children,
  style,
}) => (
  <AbsoluteFill
    style={{alignItems: 'center', justifyContent: 'center', flexDirection: 'column', ...style}}
  >
    {children}
  </AbsoluteFill>
);

/* The kaspulse pulse-waveform sigil. `draw` (0→1) reveals the stroke; `glow`
   toggles the teal bloom. Same path as the site favicon/nav sigil. */
export const PulseSigil: React.FC<{size?: number; draw?: number; glow?: boolean; color?: string}> = ({
  size = 64,
  draw = 1,
  glow = true,
  color = T.teal,
}) => {
  const L = 200; // approx path length for dash reveal
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" style={{filter: glow ? 'drop-shadow(0 0 10px rgba(73,234,203,0.6))' : 'none'}}>
      <path
        d="M4 34h13l6-19 9 34 6-24 4 13h14"
        fill="none"
        stroke={color}
        strokeWidth={4}
        strokeLinejoin="round"
        strokeLinecap="round"
        strokeDasharray={L}
        strokeDashoffset={L * (1 - Math.max(0, Math.min(1, draw)))}
      />
    </svg>
  );
};

/* kaspulse wordmark (the "u" carries a teal pulse dot). */
export const Wordmark: React.FC<{size?: number; withSigil?: boolean}> = ({size = 48, withSigil = true}) => (
  <div style={{display: 'flex', alignItems: 'center', gap: size * 0.32, fontFamily: T.sans}}>
    {withSigil && <PulseSigil size={size * 1.05} />}
    <div style={{fontSize: size, fontWeight: 800, letterSpacing: -1.5, color: T.ink}}>
      kasp<span style={{color: T.teal, textShadow: T.tealGlow}}>u</span>lse
    </div>
  </div>
);

/* Uppercase mono tracked label. */
export const Label: React.FC<{children: React.ReactNode; color?: string; style?: React.CSSProperties}> = ({
  children,
  color = T.inkDim,
  style,
}) => (
  <div style={{fontSize: 24, letterSpacing: 5, textTransform: 'uppercase', color, fontWeight: 500, ...style}}>
    {children}
  </div>
);

/* A card panel. */
export const Panel: React.FC<{children: React.ReactNode; style?: React.CSSProperties}> = ({children, style}) => (
  <div
    style={{
      background: T.panel,
      border: `1px solid ${T.panelEdge}`,
      borderRadius: 14,
      boxShadow: '0 40px 120px rgba(0,0,0,0.5)',
      ...style,
    }}
  >
    {children}
  </div>
);

/* Hairline divider. */
export const Rule: React.FC<{width?: number | string; opacity?: number}> = ({width = '100%', opacity = 1}) => (
  <div style={{width, height: 0, borderTop: `1px solid ${T.panelEdge}`, opacity}} />
);

/* An animated teal checkmark. `lit` 0→1 draws + glows it. */
export const Check: React.FC<{lit?: number; size?: number; color?: string}> = ({lit = 1, size = 30, color = T.teal}) => {
  const L = 24;
  const p = Math.max(0, Math.min(1, lit));
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" style={{opacity: 0.3 + 0.7 * p}}>
      <path
        d="M4 12.5l5 5L20 6"
        fill="none"
        stroke={color}
        strokeWidth={3}
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeDasharray={L}
        strokeDashoffset={L * (1 - p)}
        style={{filter: p > 0.9 ? 'drop-shadow(0 0 6px rgba(73,234,203,0.7))' : 'none'}}
      />
    </svg>
  );
};

/* A cross (for competitor rows / ✗). */
export const Cross: React.FC<{size?: number}> = ({size = 26}) => (
  <svg width={size} height={size} viewBox="0 0 24 24">
    <path d="M6 6l12 12M18 6L6 18" fill="none" stroke={T.red} strokeWidth={3} strokeLinecap="round" opacity={0.8} />
  </svg>
);

/* A live feed board row: pair · price · signed badge. */
export const FeedRow: React.FC<{pair: string; price: string; sources?: number; opacity?: number; featured?: boolean}> = ({
  pair,
  price,
  sources,
  opacity = 1,
  featured = false,
}) => (
  <div
    style={{
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 28,
      padding: featured ? '22px 30px' : '16px 30px',
      background: featured ? `${T.teal}0e` : T.panel,
      border: `1px solid ${featured ? T.tealDim + '66' : T.panelEdge}`,
      borderRadius: 12,
      opacity,
      minWidth: 620,
    }}
  >
    <div style={{fontFamily: T.mono, fontWeight: 700, fontSize: featured ? 34 : 26, color: T.ink}}>{pair}</div>
    <div style={{display: 'flex', alignItems: 'center', gap: 22}}>
      <div style={{fontFamily: T.mono, fontSize: featured ? 34 : 26, fontWeight: 600, color: T.teal, fontVariantNumeric: 'tabular-nums'}}>{price}</div>
      <div style={{display: 'flex', alignItems: 'center', gap: 8, color: T.inkDim, fontSize: 18}}>
        <Check lit={1} size={20} />
        <span style={{fontFamily: T.mono}}>3-of-5</span>
      </div>
    </div>
  </div>
);

/* A small KRC-20 token chip (the flex wall). */
export const TokenChip: React.FC<{name: string; opacity?: number; scale?: number}> = ({name, opacity = 1, scale = 1}) => (
  <div
    style={{
      display: 'flex',
      alignItems: 'center',
      gap: 12,
      padding: '12px 18px',
      background: T.panel,
      border: `1px solid ${T.panelEdge}`,
      borderRadius: 999,
      opacity,
      transform: `scale(${scale})`,
    }}
  >
    <PulseSigil size={22} glow={false} color={T.violet} />
    <span style={{fontFamily: T.mono, fontSize: 24, fontWeight: 600, color: T.ink}}>{name}</span>
    <span style={{fontFamily: T.mono, fontSize: 18, color: T.teal}}>/USD</span>
  </div>
);

/* A committee signer row: index · pubkey prefix · animated check. */
export const SigRow: React.FC<{i: number; pk: string; lit?: number}> = ({i, pk, lit = 0}) => (
  <div
    style={{
      display: 'flex',
      alignItems: 'center',
      gap: 22,
      padding: '14px 26px',
      background: T.panel,
      border: `1px solid ${lit > 0.5 ? T.tealDim + '66' : T.panelEdge}`,
      borderRadius: 12,
      minWidth: 560,
    }}
  >
    <Label style={{fontSize: 20, letterSpacing: 3, color: T.inkFaint}}>node {i}</Label>
    <span style={{fontFamily: T.mono, fontSize: 24, color: T.inkDim, flex: 1}}>{pk}</span>
    <Check lit={lit} size={26} />
  </div>
);

/* Blinking block cursor. */
export const Cursor: React.FC<{on: boolean; height?: number; color?: string}> = ({on, height = 34, color = T.teal}) => (
  <span
    style={{
      display: 'inline-block',
      width: 14,
      height,
      background: color,
      opacity: on ? 1 : 0,
      transform: 'translateY(4px)',
      boxShadow: on ? T.tealGlow : 'none',
    }}
  />
);
