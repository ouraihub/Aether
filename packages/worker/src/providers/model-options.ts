import { type ModelFilter, type ModelOption } from '@ouraihub/aether-contracts'
import type { Catalog } from './catalog.js'

export function modelOptions(catalog: Catalog, filter: ModelFilter): ModelOption[] {
  return catalog.providers
    .filter((provider) => provider.enabled)
    .flatMap((provider) =>
      provider.endpoints.flatMap((endpoint) =>
        endpoint.models
          .filter(
            (model) =>
              (filter.modelType === undefined || model.modelType === filter.modelType) &&
              (filter.supportsTools === undefined || model.supportsTools === filter.supportsTools),
          )
          .map((model) => ({
            endpointId: endpoint.id,
            modelId: model.id,
            label: `${provider.name} · ${endpoint.slug} · ${model.id}`,
            name: model.name,
            modelType: model.modelType,
            supportsTools: model.supportsTools,
            reasoning: model.reasoning,
            nativeWebSearch: model.nativeWebSearch,
            canVision: model.canVision,
          })),
      ),
    )
}
