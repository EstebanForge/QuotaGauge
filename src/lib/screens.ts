export type AppScreen = 'dashboard' | 'customize' | 'settings' | `provider:${string}`;

export function isProviderScreen(screen: AppScreen) {
  return screen.startsWith('provider:');
}

export function providerIdFromScreen(screen: AppScreen) {
  return isProviderScreen(screen) ? screen.slice('provider:'.length) : null;
}
