import appSource from '../App.svelte?raw';
import customizeDetailSource from './CustomizeProviderDetail.svelte?raw';
import customizeListSource from './CustomizeProviderList.svelte?raw';
import dashboardSource from './Dashboard.svelte?raw';
import iconSource from './Icon.svelte?raw';
import providerIconSource from './ProviderIcon.svelte?raw';
import quotaMetricSource from './QuotaMetric.svelte?raw';
import selectMenuSource from './SelectMenu.svelte?raw';
import settingsSource from './SettingsScreen.svelte?raw';

const componentSources = [
  appSource,
  customizeDetailSource,
  customizeListSource,
  dashboardSource,
  iconSource,
  providerIconSource,
  quotaMetricSource,
  selectMenuSource,
  settingsSource,
];

function extractStyleBlocks(source: string): string[] {
  return Array.from(source.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g), (match) => match[1]);
}

export const coLocatedComponentCss = componentSources.flatMap(extractStyleBlocks).join('\n');
