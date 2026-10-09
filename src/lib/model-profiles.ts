import type { Config, ModelConfig, ModelProfile, ReasoningEffort } from '@/types'

export const DEFAULT_MODEL_CONFIG: ModelConfig = {
  provider: 'deepseek', name: 'deepseek-chat', apiKey: null, baseUrl: null, temperature: 0.7, maxTokens: null,
}

export type ModelOptions = Pick<ModelProfile, 'supportsReasoning' | 'reasoningEffort' | 'thinking'>

/** Shared by both editors; preserve compatible options and clear unsupported ones. */
export function updateModelProfile(profile: ModelProfile, config: ModelConfig): ModelProfile {
  const next = { ...profile, config }
  if (!['openai', 'newapi'].includes(config.provider)) next.supportsReasoning = false
  if (!supportsThinkingToggle(config.provider, config.name)) next.thinking = null
  if (!reasoningEfforts(next).includes(next.reasoningEffort!)) next.reasoningEffort = null
  return next
}

export function tokenParameter(profile: ModelProfile): string {
  return ['openai', 'newapi'].includes(profile.config.provider) && profile.supportsReasoning
    ? 'max_completion_tokens' : 'max_tokens'
}

export function temperatureEnabled(profile: ModelProfile): boolean {
  return !(['openai', 'newapi'].includes(profile.config.provider) && profile.supportsReasoning
    || profile.config.provider === 'deepseek' && ['deepseek-flash', 'deepseek-v4-pro', 'deepseek-reasoner'].includes(profile.config.name) && profile.thinking !== false)
}

export const EFFORT_LABELS = { low: '低', medium: '中', high: '高', max: '最高' } as const

export function reasoningEfforts(profile: ModelProfile): ReasoningEffort[] {
  if (profile.thinking === false) return []
  if (['openai', 'newapi'].includes(profile.config.provider) && profile.supportsReasoning) return ['low', 'medium', 'high']
  if (profile.config.provider === 'deepseek' && ['deepseek-flash', 'deepseek-v4-pro'].includes(profile.config.name)) return ['low', 'high', 'max']
  return []
}

export function supportsThinkingToggle(provider: string, model: string): boolean {
  return provider === 'deepseek' && ['deepseek-flash', 'deepseek-v4-pro'].includes(model)
    || provider === 'qwen' && ['qwen3-235b-a22b', 'qwen3-30b-a3b', 'qwen3-32b', 'qwen3-14b', 'qwen3-8b', 'qwen3-4b', 'qwen3-1.7b', 'qwen3-0.6b'].includes(model)
}

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
