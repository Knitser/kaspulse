import {Easing, interpolate, spring} from 'remotion';

/* 0→1 over [from, to], clamped, eased (default: cubic out). */
export const fade = (
  frame: number,
  from: number,
  to: number,
  easing: (t: number) => number = Easing.out(Easing.cubic)
): number =>
  interpolate(frame, [from, to], [0, 1], {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
    easing,
  });

/* Arbitrary clamped interpolation with cubic-out easing. */
export const glide = (
  frame: number,
  range: [number, number],
  out: [number, number],
  easing: (t: number) => number = Easing.out(Easing.cubic)
): number =>
  interpolate(frame, range, out, {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
    easing,
  });

/* Overshoot pop (stamp/impact) — springy 0→1. Pass fps from useVideoConfig. */
export const pop = (frame: number, delay: number, fps: number, damping = 12): number =>
  spring({frame: frame - delay, fps, config: {damping, mass: 0.7, stiffness: 140}});

/* Count-up: eased number from 0→value over [from,to]. */
export const countTo = (frame: number, from: number, to: number, value: number): number =>
  interpolate(frame, [from, to], [0, value], {
    extrapolateLeft: 'clamp',
    extrapolateRight: 'clamp',
    easing: Easing.out(Easing.cubic),
  });

/* Blinking cursor helper: 1 during on-phase, 0 during off. */
export const blink = (frame: number, period = 30): number =>
  (frame % period) < period / 2 ? 1 : 0;

/* Typewriter: how many chars of `text` are visible at `frame`, starting at
   `from`, at `cps` chars per second (needs fps). */
export const typed = (frame: number, from: number, fps: number, text: string, cps = 30): string => {
  const n = Math.max(0, Math.floor(((frame - from) / fps) * cps));
  return text.slice(0, Math.min(text.length, n));
};
