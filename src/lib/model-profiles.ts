import type { Config, ModelProfile } from '@/types'

export const EFFORT_LABELS = { low: '低', medium: '中', high: '高' } as const

export function modelProfiles(config: Config | null): ModelProfile[] {
  if (!config) return []
  return config.modelProfiles?.length ? config.modelProfiles : [{
    id: 'legacy-model', label: config.model.name, config: config.model,
    supportsReasoning: false, reasoningEffort: null,
  }]
}

export function activeProfile(config: Config | null): ModelProfile | undefined {
  const profiles = modelProfiles(config)
  return profiles.find((profile) => profile.id === config?.activeModelId) ?? profiles[0]
}
