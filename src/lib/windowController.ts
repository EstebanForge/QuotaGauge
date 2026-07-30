export type AppScreen = 'dashboard' | 'customize' | 'settings' | `provider:${string}`;

interface WindowControllerOptions {
  screen: () => AppScreen;
  refreshing: () => boolean;
  reordering: () => boolean;
}

export function createWindowController(options: WindowControllerOptions) {
  let measureFrame = 0;

  function shouldDefer() {
    return options.reordering() || (options.screen() === 'dashboard' && options.refreshing());
  }

  function scheduleFit() {
    if (typeof window === 'undefined') return;
    if (shouldDefer()) {
      window.cancelAnimationFrame(measureFrame);
      return;
    }
    window.cancelAnimationFrame(measureFrame);
    measureFrame = window.requestAnimationFrame(measurePage);
  }

  function measurePage() {
    if (shouldDefer()) return;
    const screen = options.screen();
    const page = document.querySelector<HTMLElement>(`.screen-page[data-screen="${screen}"]`);
    const stage = document.querySelector<HTMLElement>('.screen-stage');
    if (!page || !stage) return;

    // The user owns the native panel height. We only keep the transition stage matched to the
    // currently rendered page so content can slide and scroll inside that fixed window.
    const renderedHeight = page.getBoundingClientRect().height;
    const pageHeight = renderedHeight > 0 ? renderedHeight : page.offsetHeight || page.scrollHeight;
    stage.style.height = `${pageHeight}px`;
  }

  return {
    beginContentMorph: scheduleFit,
    scheduleFit,
    dispose() {
      window.cancelAnimationFrame(measureFrame);
    },
  };
}
