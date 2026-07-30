import { cleanup, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createWindowController } from './windowController';

describe('window controller with a user-sized panel', () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <main class="content">
        <div class="screen-stage">
          <div class="screen-page" data-screen="dashboard"></div>
        </div>
      </main>
    `;
  });

  afterEach(() => {
    cleanup();
    document.body.innerHTML = '';
  });

  it('measures the page without resizing the native window', async () => {
    const page = document.querySelector<HTMLElement>('.screen-page')!;
    page.getBoundingClientRect = vi.fn(
      () =>
        ({
          width: 292,
          height: 430,
          top: 0,
          right: 292,
          bottom: 430,
          left: 0,
          x: 0,
          y: 0,
        }) as DOMRect,
    );
    const controller = createWindowController({
      screen: () => 'dashboard',
      refreshing: () => false,
      reordering: () => false,
    });

    controller.beginContentMorph();

    await waitFor(() =>
      expect(document.querySelector<HTMLElement>('.screen-stage')).toHaveStyle({ height: '430px' }),
    );
    controller.dispose();
  });

  it('defers measurements while provider refresh or reordering can move rows', async () => {
    const page = document.querySelector<HTMLElement>('.screen-page')!;
    page.getBoundingClientRect = vi.fn(
      () =>
        ({
          width: 292,
          height: 360,
          top: 0,
          right: 292,
          bottom: 360,
          left: 0,
          x: 0,
          y: 0,
        }) as DOMRect,
    );
    let refreshing = true;
    const controller = createWindowController({
      screen: () => 'dashboard',
      refreshing: () => refreshing,
      reordering: () => false,
    });

    controller.scheduleFit();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(document.querySelector<HTMLElement>('.screen-stage')?.style.height).toBe('');

    refreshing = false;
    controller.scheduleFit();
    await waitFor(() =>
      expect(document.querySelector<HTMLElement>('.screen-stage')).toHaveStyle({ height: '360px' }),
    );
    controller.dispose();
  });
});
