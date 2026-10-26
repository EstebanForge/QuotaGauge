import { describe, expect, it } from 'vitest';
import { isProviderScreen, providerIdFromScreen } from './screens';

describe('screens', () => {
  it('detects provider screens', () => {
    expect(isProviderScreen('provider:claude')).toBe(true);
    expect(isProviderScreen('dashboard')).toBe(false);
    expect(isProviderScreen('settings')).toBe(false);
    expect(isProviderScreen('customize')).toBe(false);
  });

  it('extracts the provider id only from provider screens', () => {
    expect(providerIdFromScreen('provider:claude')).toBe('claude');
    expect(providerIdFromScreen('provider:z ai')).toBe('z ai');
    expect(providerIdFromScreen('provider:')).toBe('');
    expect(providerIdFromScreen('dashboard')).toBeNull();
    expect(providerIdFromScreen('customize')).toBeNull();
  });
});
