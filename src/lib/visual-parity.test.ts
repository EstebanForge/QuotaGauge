import { cleanup, render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ProviderIcon from './ProviderIcon.svelte';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('native visual contract', () => {
  it.each([
    'claude',
    'codex',
    'cursor',
    'antigravity',
    'copilot',
    'devin',
    'grok',
    'opencode',
    'openrouter',
    'zai',
  ])('packages the exact %s provider icon', (providerId) => {
    const { container } = render(ProviderIcon, { providerId });
    const icon = container.querySelector('.provider-icon');
    expect(icon).not.toBeNull();
    const path = icon?.querySelector('path')?.getAttribute('d');
    expect(path?.length).toBeGreaterThan(10);
  });

  it('renders provider marks with their intended brand treatment', () => {
    const claude = render(ProviderIcon, { providerId: 'claude' });
    expect(claude.container.querySelector('path')).toHaveAttribute('fill', '#DE7356');
    cleanup();
    const codex = render(ProviderIcon, { providerId: 'codex' });
    expect(codex.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
    cleanup();
    const copilot = render(ProviderIcon, { providerId: 'copilot' });
    expect(copilot.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
    cleanup();
    const cursor = render(ProviderIcon, { providerId: 'cursor' });
    expect(cursor.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
    cleanup();
    const devin = render(ProviderIcon, { providerId: 'devin' });
    expect(devin.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
    cleanup();
    const antigravity = render(ProviderIcon, { providerId: 'antigravity' });
    expect(antigravity.container.querySelector('path')).toHaveAttribute('fill', '#4285F4');
    cleanup();
    const grok = render(ProviderIcon, { providerId: 'grok' });
    expect(grok.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
    cleanup();
    const opencode = render(ProviderIcon, { providerId: 'opencode' });
    expect(opencode.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
    cleanup();
    const openrouter = render(ProviderIcon, { providerId: 'openrouter' });
    expect(openrouter.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
    cleanup();
    const zai = render(ProviderIcon, { providerId: 'zai' });
    expect(zai.container.querySelector('path')).toHaveAttribute('fill', 'currentColor');
  });

  it('reuses the Claude mark for Claude account cards', () => {
    const claude = render(ProviderIcon, { providerId: 'claude' });
    const account = render(ProviderIcon, { providerId: 'claude@1234abcd' });

    expect(account.container.innerHTML).toBe(claude.container.innerHTML);
  });
});
