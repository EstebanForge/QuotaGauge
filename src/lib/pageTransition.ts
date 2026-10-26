import type { EasingFunction, TransitionConfig } from 'svelte/transition';
import type { AppScreen } from './screens';

interface PageTransitionOptions {
  direction: number;
  duration: number;
  easing: EasingFunction;
}

export function shouldSlideBetweenScreens(from: AppScreen, to: AppScreen) {
  return from !== to;
}

export function horizontalPageTransition(
  _node: Element,
  { direction, duration, easing }: PageTransitionOptions,
): TransitionConfig {
  const distance = _node.getBoundingClientRect().width * direction;
  return {
    duration,
    easing,
    css: (t) => `transform: translate3d(${(1 - t) * distance}px, 0, 0);`,
  };
}
