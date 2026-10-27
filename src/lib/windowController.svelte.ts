import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import {
  beginPanelResize,
  fitPanelToContent,
  getPanelHeightMode,
  getPanelResizeEdge,
  lockPanelResizeAxis,
  setPanelHeightAutomatic,
  setPanelHeightManual,
} from './backend';
import type { PanelHeightMode, PanelResizeEdge } from './backend';
import { springMotion } from './motion';
import { panelTargetHeight, screenPanelHeight, shouldDeferPanelFit } from './panelSizing';
import type { AppScreen } from './screens';

interface WindowControllerOptions {
  screen: () => AppScreen;
  refreshing: () => boolean;
  reordering: () => boolean;
  floating: () => boolean;
  reducedMotion: () => boolean;
  initialResizeEdge: PanelResizeEdge;
  onError: (message: string) => void;
}

function cssPixels(value: string) {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

export function createWindowController(options: WindowControllerOptions) {
  let measureFrame = 0;
  let resizeFrame = 0;
  let resizeGeneration = 0;
  let resizeAvailable = true;
  let contentMorphActive = false;
  let contentMorphTimer: ReturnType<typeof setTimeout> | undefined;
  let resizeInFlight = false;
  let pendingResizeHeight: number | null = null;
  let dashboardBodyHeight: number | null = null;
  let lastKeyboardActivity = 0;

  // Panel sizing: the resize edge, the manual/automatic height mode and the drag grip
  // choreography. All native calls funnel through here so App.svelte only renders.
  let resizeEdge = $state<PanelResizeEdge>(options.initialResizeEdge);
  let panelHeightMode = $state<PanelHeightMode>('automatic');
  let panelHeightModeRequest = 0;
  let panelHeightModeMutation: Promise<void> = Promise.resolve();
  let lastResizeGripPointerAt = Number.NEGATIVE_INFINITY;
  let panelResizeOperation: Promise<void> | null = null;
  const renderedResizeEdge = $derived(options.floating() ? 'bottom' : resizeEdge);

  // macOS WebKit seeds keyboard-style focus to the first tabbable element (the first card's drag
  // grip) when the panel window resizes, painting a stray focus ring for pointer users.
  function dropSeededHandleFocus() {
    if (typeof window === 'undefined') return;
    if (performance.now() - lastKeyboardActivity < 500) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && active.matches('[data-reorder-touch-handle]')) {
      active.blur();
    }
  }

  if (typeof window !== 'undefined') {
    window.addEventListener('keydown', markKeyboardActivity, { capture: true });
  }

  function markKeyboardActivity() {
    lastKeyboardActivity = performance.now();
  }

  function shouldDefer() {
    return options.reordering() || shouldDeferPanelFit(options.screen(), options.refreshing());
  }

  function cancelPendingResize() {
    if (typeof window === 'undefined') return;
    window.cancelAnimationFrame(resizeFrame);
    pendingResizeHeight = null;
    resizeGeneration += 1;
  }

  function beginContentMorph() {
    if (typeof window === 'undefined') return;
    cancelPendingResize();
    window.clearTimeout(contentMorphTimer);
    contentMorphActive = panelHeightMode === 'automatic' && !options.reducedMotion();
    if (!contentMorphActive) {
      scheduleFit();
      return;
    }
    const duration = springMotion(false).duration;
    contentMorphTimer = window.setTimeout(() => {
      contentMorphActive = false;
      scheduleFit();
      window.requestAnimationFrame(dropSeededHandleFocus);
    }, duration + 34);
  }

  function scheduleFit() {
    if (typeof window === 'undefined') return;
    if (shouldDefer()) {
      window.cancelAnimationFrame(measureFrame);
      cancelPendingResize();
      return;
    }
    window.cancelAnimationFrame(measureFrame);
    measureFrame = window.requestAnimationFrame(() => void fit());
  }

  async function fit() {
    if (shouldDefer()) return;
    const screen = options.screen();
    const page = document.querySelector<HTMLElement>(`.screen-page[data-screen="${screen}"]`);
    const content = document.querySelector<HTMLElement>('.content');
    const stage = document.querySelector<HTMLElement>('.screen-stage');
    const header = document.querySelector<HTMLElement>('.screen-header');
    const footer = document.querySelector<HTMLElement>('.footer');
    const dragger = document.querySelector<HTMLElement>('.panel-resize-dragger');
    const floatingChrome = document.querySelector<HTMLElement>('.floating-chrome');
    if (!page || !content || !stage) return;

    const renderedHeight = page.getBoundingClientRect().height;
    const pageHeight = renderedHeight > 0 ? renderedHeight : page.offsetHeight || page.scrollHeight;
    stage.style.height = `${pageHeight}px`;

    if (panelHeightMode !== 'automatic' || !('__TAURI_INTERNALS__' in window) || !resizeAvailable)
      return;

    const contentStyle = window.getComputedStyle(content);
    const contentPadding =
      cssPixels(contentStyle.paddingTop) + cssPixels(contentStyle.paddingBottom);
    const chromeHeight = floatingChrome?.offsetHeight ?? 0;
    const idealHeight =
      pageHeight +
      contentPadding +
      (header?.offsetHeight ?? 0) +
      (footer?.offsetHeight ?? 0) +
      (dragger?.offsetHeight ?? 0) +
      chromeHeight;
    const monitor = await currentMonitor().catch(() => null);
    const workAreaHeight = monitor
      ? monitor.workArea.size.height / monitor.scaleFactor
      : window.screen.availHeight;
    const contentTarget = panelTargetHeight(idealHeight, workAreaHeight);
    if (screen === 'dashboard') dashboardBodyHeight = contentTarget - chromeHeight;

    let current: number | null = null;
    if (screen === 'settings' && dashboardBodyHeight === null) {
      const appWindow = getCurrentWindow();
      const scale = await appWindow.scaleFactor();
      current = (await appWindow.innerSize()).height / scale;
    }
    const dashboardTarget = panelTargetHeight(
      (dashboardBodyHeight ?? Math.round(current ?? contentTarget) - chromeHeight) + chromeHeight,
      workAreaHeight,
    );
    const target = screenPanelHeight(screen, contentTarget, dashboardTarget);

    if (options.reducedMotion() || contentMorphActive) {
      ++resizeGeneration;
      window.cancelAnimationFrame(resizeFrame);
      await resize(target);
      return;
    }

    if (current === null) {
      const appWindow = getCurrentWindow();
      const scale = await appWindow.scaleFactor();
      current = (await appWindow.innerSize()).height / scale;
    }
    const generation = ++resizeGeneration;
    window.cancelAnimationFrame(resizeFrame);
    if (Math.abs(current - target) < 1) return;

    const started = performance.now();
    const motion = springMotion(false);
    const animate = (time: number) => {
      if (generation !== resizeGeneration || panelHeightMode !== 'automatic') return;
      const progress = Math.min(1, (time - started) / motion.duration);
      const eased = motion.easing(progress);
      void resize(Math.round(current + (target - current) * eased));
      if (progress < 1) resizeFrame = window.requestAnimationFrame(animate);
    };
    resizeFrame = window.requestAnimationFrame(animate);
  }

  async function resize(height: number) {
    pendingResizeHeight = height;
    if (resizeInFlight) return;
    resizeInFlight = true;
    try {
      while (pendingResizeHeight !== null && resizeAvailable && panelHeightMode === 'automatic') {
        const nextHeight = pendingResizeHeight;
        pendingResizeHeight = null;
        await fitPanelToContent(nextHeight);
      }
    } catch {
      pendingResizeHeight = null;
      resizeAvailable = false;
      options.onError('QuotaGauge window could not adapt to its content.');
    } finally {
      resizeInFlight = false;
      if (pendingResizeHeight === null) {
        window.requestAnimationFrame(() => window.requestAnimationFrame(dropSeededHandleFocus));
      }
    }
  }

  function updateResizeEdge() {
    if (!('__TAURI_INTERNALS__' in window)) return;
    void getPanelResizeEdge()
      .then((edge) => (resizeEdge = edge))
      .catch(() => undefined);
  }

  function updateHeightMode() {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const request = ++panelHeightModeRequest;
    void getPanelHeightMode()
      .then((mode) => {
        if (request !== panelHeightModeRequest) return;
        panelHeightMode = mode;
        if (mode === 'automatic') scheduleFit();
      })
      .catch(() => undefined);
  }

  function acceptHeightMode(mode: PanelHeightMode) {
    panelHeightModeRequest += 1;
    panelHeightMode = mode;
  }

  function handlePanelResizePointerDown(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const pointerAt = event.timeStamp;
    const repeatedPress = event.detail > 1 || pointerAt - lastResizeGripPointerAt <= 400;
    lastResizeGripPointerAt = repeatedPress ? Number.NEGATIVE_INFINITY : pointerAt;
    if (repeatedPress) {
      const activeResize = panelResizeOperation;
      void (async () => {
        if (activeResize) await activeResize;
        await changeHeightMode('automatic');
      })();
      return;
    }
    const operation = (async () => {
      try {
        await panelHeightModeMutation;
        const edge = await beginPanelResize();
        resizeEdge = edge;
        // The native begin command has already persisted the current height as manual. Mirroring it
        // here stops any in-flight frontend auto-fit without waiting for the first resize event.
        acceptHeightMode('manual');
        // TODO(macOS): Tao 0.35 reports native resize dragging as unsupported and Tauri currently
        // swallows that runtime error. Re-test after Tauri/Tao upgrades; add an AppKit fallback if
        // upstream support is still unavailable.
        await getCurrentWindow().startResizeDragging(edge === 'top' ? 'North' : 'South');
      } catch {
        options.onError('QuotaGauge panel resize could not be started.');
      } finally {
        await lockPanelResizeAxis().catch(() => undefined);
        updateHeightMode();
      }
    })();
    panelResizeOperation = operation;
    void operation.finally(() => {
      if (panelResizeOperation === operation) panelResizeOperation = null;
    });
  }

  function handleFloatingWindowPointerDown(event: PointerEvent) {
    if (event.button !== 0 || !('__TAURI_INTERNALS__' in window)) return;
    event.preventDefault();
    void getCurrentWindow()
      .startDragging()
      .catch(() => options.onError('QuotaGauge window could not be moved.'));
  }

  async function changeHeightMode(mode: PanelHeightMode) {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const request = ++panelHeightModeRequest;
    const operation = panelHeightModeMutation.then(() =>
      mode === 'automatic' ? setPanelHeightAutomatic() : setPanelHeightManual(),
    );
    panelHeightModeMutation = operation.catch(() => undefined);
    try {
      await operation;
      if (request === panelHeightModeRequest) updateHeightMode();
    } catch {
      if (request !== panelHeightModeRequest) return;
      options.onError('QuotaGauge could not change the panel height mode.');
      updateHeightMode();
    }
  }

  return {
    beginContentMorph,
    scheduleFit,
    sizing: {
      get mode() {
        return panelHeightMode;
      },
      get resizeEdge() {
        return renderedResizeEdge;
      },
      get settled() {
        return panelHeightModeMutation;
      },
      updateEdge: updateResizeEdge,
      updateMode: updateHeightMode,
      acceptMode: acceptHeightMode,
      changeMode: changeHeightMode,
      resizePointerDown: handlePanelResizePointerDown,
      floatingPointerDown: handleFloatingWindowPointerDown,
    },
    dispose() {
      window.clearTimeout(contentMorphTimer);
      contentMorphActive = false;
      window.cancelAnimationFrame(measureFrame);
      cancelPendingResize();
      window.removeEventListener('keydown', markKeyboardActivity, { capture: true });
    },
  };
}
