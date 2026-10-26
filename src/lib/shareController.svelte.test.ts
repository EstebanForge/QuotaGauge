import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createShareController } from './shareController.svelte';

describe('shareController', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('shows a message and clears it after the toast window', () => {
    const share = createShareController();
    expect(share.message).toBeNull();
    share.show('Copied to clipboard');
    expect(share.message).toBe('Copied to clipboard');
    vi.advanceTimersByTime(1799);
    expect(share.message).toBe('Copied to clipboard');
    vi.advanceTimersByTime(1);
    expect(share.message).toBeNull();
  });

  it('replaces a pending toast and cancels the old timer', () => {
    const share = createShareController();
    share.show('First');
    vi.advanceTimersByTime(1000);
    share.show('Second');
    vi.advanceTimersByTime(1000);
    expect(share.message).toBe('Second');
    vi.advanceTimersByTime(800);
    expect(share.message).toBeNull();
  });

  it('dismisses immediately and cancels the pending clear', () => {
    const share = createShareController();
    share.show('Copied to clipboard');
    share.dismiss();
    expect(share.message).toBeNull();
    vi.advanceTimersByTime(5000);
    expect(share.message).toBeNull();
  });
});
