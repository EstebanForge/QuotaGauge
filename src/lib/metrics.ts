import type { MetricDefinition, ProviderCatalog, ProviderDefinition } from './types';

export class ProviderCatalogIndex {
  readonly providers: ProviderDefinition[];
  readonly #providersById: Map<string, ProviderDefinition>;
  readonly #metricsById: Map<string, MetricDefinition>;
  readonly #apiKeyProviderIds: Set<string>;

  constructor(catalog: ProviderCatalog) {
    this.providers = catalog.providers;
    this.#providersById = new Map();
    this.#metricsById = new Map();
    this.#apiKeyProviderIds = new Set(catalog.apiKeyProviderIds ?? []);

    for (const provider of catalog.providers) {
      if (this.#providersById.has(provider.id)) {
        throw new Error(`Duplicate provider definition: ${provider.id}`);
      }
      this.#providersById.set(provider.id, provider);
      for (const metric of provider.metrics) {
        if (this.#metricsById.has(metric.id)) {
          throw new Error(`Duplicate metric definition: ${metric.id}`);
        }
        this.#metricsById.set(metric.id, metric);
      }
    }
  }

  provider(id: string) {
    return this.#providersById.get(id);
  }

  metric(id: string) {
    return this.#metricsById.get(id);
  }

  displayName(id: string, providerNames?: Record<string, string>) {
    const customName = providerNames?.[id]?.trim();
    if (customName) return customName;
    return this.provider(id)?.displayName ?? id;
  }

  supportsApiKeyConfiguration(id: string) {
    return this.#apiKeyProviderIds.has(id);
  }
}

export const emptyProviderCatalog = new ProviderCatalogIndex({ providers: [] });
