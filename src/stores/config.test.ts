import { beforeEach, describe, expect, it, vi } from 'vitest'

import { ipc } from '@/services/ipc'
import { useConfigStore } from '@/stores/config'
import type { Config, ModelConfig, ModelProfile } from '@/types'

vi.mock('@/services/ipc', () => ({
  ipc: {
    getConfig: vi.fn(),
    setConfig: vi.fn(),
    getModelConfig: vi.fn(),
    setModelConfig: vi.fn(),
    testModelConnection: vi.fn(),
  },
}))

const mockedIpc = vi.mocked(ipc, true)

const modelConfig: ModelConfig = {
  provider: 'openai',
  name: 'gpt-4o-mini',
  apiKey: 'sk-test',
  baseUrl: null,
  temperature: 0.2,
  maxTokens: 4096,
}

const strongModel: ModelConfig = {
  provider: 'anthropic',
  name: 'claude-sonnet',
  apiKey: 'sk-ant',
  baseUrl: null,
  temperature: 0.4,
  maxTokens: 8192,
}

const config: Config = {
  model: modelConfig,
  router: { fastModel: 'gpt-4o-mini', strongModel: 'claude-sonnet' },
}

describe('useConfigStore', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useConfigStore.setState({ config: null, modelConfig: null, loading: false })
  })

  it('has empty initial state', () => {
    const state = useConfigStore.getState()
    expect(state.config).toBeNull()
    expect(state.modelConfig).toBeNull()
    expect(state.loading).toBe(false)
  })

  describe('loadConfig', () => {
    it('stores the config and syncs the modelConfig view', async () => {
      mockedIpc.getConfig.mockResolvedValue(config)

      await useConfigStore.getState().loadConfig()

      expect(mockedIpc.getConfig).toHaveBeenCalledTimes(1)
      const state = useConfigStore.getState()
      expect(state.config).toEqual(config)
      expect(state.modelConfig).toEqual(modelConfig)
      expect(state.loading).toBe(false)
    })

    it('exposes loading while loading and resets it on failure', async () => {
      let resolveGet!: (c: Config) => void
      mockedIpc.getConfig.mockImplementation(
        () => new Promise<Config>((resolve) => (resolveGet = resolve)),
      )

      const pending = useConfigStore.getState().loadConfig()
      expect(useConfigStore.getState().loading).toBe(true)

      resolveGet(config)
      await pending
      expect(useConfigStore.getState().loading).toBe(false)
    })

    it('keeps state untouched on failure', async () => {
      mockedIpc.getConfig.mockRejectedValue(new Error('backend gone'))

      await expect(useConfigStore.getState().loadConfig()).rejects.toThrow('backend gone')

      const state = useConfigStore.getState()
      expect(state.config).toBeNull()
      expect(state.loading).toBe(false)
    })
  })

  describe('saveConfig', () => {
    it('persists to the backend and updates state', async () => {
      mockedIpc.setConfig.mockResolvedValue(undefined)

      await useConfigStore.getState().saveConfig(config)

      expect(mockedIpc.setConfig).toHaveBeenCalledWith(config)
      const state = useConfigStore.getState()
      expect(state.config).toEqual(config)
      expect(state.modelConfig).toEqual(modelConfig)
      expect(state.loading).toBe(false)
    })

    it('resets loading and keeps old config on failure', async () => {
      useConfigStore.setState({ config })
      mockedIpc.setConfig.mockRejectedValue(new Error('write failed'))

      const next: Config = { ...config, model: strongModel }
      await expect(useConfigStore.getState().saveConfig(next)).rejects.toThrow('write failed')

      const state = useConfigStore.getState()
      expect(state.config).toEqual(config)
      expect(state.loading).toBe(false)
    })
  })

  describe('loadModelConfig', () => {
    it('stores only the model config', async () => {
      mockedIpc.getModelConfig.mockResolvedValue(strongModel)

      await useConfigStore.getState().loadModelConfig()

      expect(mockedIpc.getModelConfig).toHaveBeenCalledTimes(1)
      expect(useConfigStore.getState().modelConfig).toEqual(strongModel)
      expect(useConfigStore.getState().config).toBeNull()
      expect(useConfigStore.getState().loading).toBe(false)
    })
  })

  describe('saveModelConfig', () => {
    it('persists and syncs modelConfig plus loaded config.model', async () => {
      useConfigStore.setState({ config })
      mockedIpc.setModelConfig.mockResolvedValue(undefined)

      await useConfigStore.getState().saveModelConfig(strongModel)

      expect(mockedIpc.setModelConfig).toHaveBeenCalledWith(strongModel)
      const state = useConfigStore.getState()
      expect(state.modelConfig).toEqual(strongModel)
      expect(state.config?.model).toEqual(strongModel)
      expect(state.loading).toBe(false)
    })

    it('works with no full config loaded', async () => {
      mockedIpc.setModelConfig.mockResolvedValue(undefined)

      await useConfigStore.getState().saveModelConfig(strongModel)

      expect(useConfigStore.getState().modelConfig).toEqual(strongModel)
      expect(useConfigStore.getState().config).toBeNull()
    })

    it('resets loading on failure', async () => {
      mockedIpc.setModelConfig.mockRejectedValue(new Error('provider unknown'))

      await expect(
        useConfigStore.getState().saveModelConfig(strongModel),
      ).rejects.toThrow('provider unknown')

      const state = useConfigStore.getState()
      expect(state.loading).toBe(false)
      expect(state.modelConfig).toBeNull()
    })
  })

  describe('testConnection', () => {
    it('returns true passthrough without touching state', async () => {
      mockedIpc.testModelConnection.mockResolvedValue(true)

      const result = await useConfigStore.getState().testConnection(modelConfig)

      expect(mockedIpc.testModelConnection).toHaveBeenCalledWith(modelConfig)
      expect(result).toBe(true)
      expect(useConfigStore.getState().loading).toBe(false)
    })

    it('returns false when the connection fails', async () => {
      mockedIpc.testModelConnection.mockResolvedValue(false)

      const result = await useConfigStore.getState().testConnection(modelConfig)

      expect(result).toBe(false)
    })
  })

  const profiles: ModelProfile[] = [
    { id: 'a', label: 'Fast', config: modelConfig, supportsReasoning: false, reasoningEffort: null },
    { id: 'b', label: 'Reasoner', config: { ...modelConfig, name: 'o3-mini', baseUrl: 'http://localhost:1234/v1' }, supportsReasoning: true, reasoningEffort: 'medium' },
  ]
  it('selects an independent model connection and routes all task stages to it', async () => {
    mockedIpc.setConfig.mockResolvedValue(undefined)
    useConfigStore.setState({ config: { ...config, modelProfiles: profiles, activeModelId: 'a' } })
    await useConfigStore.getState().selectModel('b', 'high')
    expect(mockedIpc.setConfig).toHaveBeenCalledWith(expect.objectContaining({ activeModelId: 'b', model: profiles[1].config,
      router: { fastModel: 'o3-mini', strongModel: 'o3-mini' }, modelProfiles: [profiles[0], { ...profiles[1], reasoningEffort: 'high' }] }))
    expect(useConfigStore.getState().modelConfig?.baseUrl).toBe('http://localhost:1234/v1')
  })
  it('preserves the previous selection when saving fails', async () => {
    const previous = { ...config, modelProfiles: profiles, activeModelId: 'a' }
    useConfigStore.setState({ config: previous })
    mockedIpc.setConfig.mockRejectedValueOnce(new Error('disk full'))
    await expect(useConfigStore.getState().selectModel('b')).rejects.toThrow('disk full')
    expect(useConfigStore.getState().config).toEqual(previous)
  })
  it('rejects unsupported effort and missing selections', async () => {
    useConfigStore.setState({ config: { ...config, modelProfiles: profiles } })
    await expect(useConfigStore.getState().selectModel('a', 'high')).rejects.toThrow('不支持')
    await expect(useConfigStore.getState().selectModel('missing')).rejects.toThrow('不存在')
    expect(mockedIpc.setConfig).not.toHaveBeenCalled()
  })
  it('migrates the original single model on its first selection', async () => {
    useConfigStore.setState({ config })
    mockedIpc.setConfig.mockResolvedValue(undefined)
    await useConfigStore.getState().selectModel('legacy-model')
    expect(useConfigStore.getState().config?.modelProfiles).toHaveLength(1)
    expect(useConfigStore.getState().config?.activeModelId).toBe('legacy-model')
  })
})
