/* Design tokens ported from the kaspulse site (web/style.css) — the
   "signed & verifiable" identity: teal signal + violet accent + gold on a
   deep near-black. JetBrains Mono for all data/signatures; Inter for display.
   Motion language is CLEAN and precise (signals, waveforms, crypto checks) —
   NOT thermal/jittery. */

export const T = {
  bg: '#080b11', // deep near-black
  bg2: '#0d1622', // gradient lift
  panel: '#101a24', // cards / rows
  panelEdge: '#1f2d3a',
  ink: '#e9f1ef', // off-white
  inkDim: '#93a6b0',
  inkFaint: '#5a6b76',
  teal: '#49eacb', // primary signal / brand / verified ✓
  tealDim: '#2ea892',
  violet: '#c792ea', // secondary accent
  gold: '#d4b463', // sparing highlight (the strike, the hero price)
  green: '#6fe0a0', // "valid" confirmations
  red: '#ff7a6b', // ✗ / halted
  mono: "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, monospace",
  sans: 'Inter, system-ui, -apple-system, "Segoe UI", sans-serif',
  margin: 96,
  tealGlow: '0 0 24px rgba(73,234,203,0.55)',
  violetGlow: '0 0 24px rgba(199,146,234,0.5)',
} as const;
