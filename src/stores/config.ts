import { create } from 'zustand'

import { ipc } from '@/services/ipc'
import type { Config, ModelConfig, ModelProfile, ReasoningEffort } from '@/types'
import { activeProfile, modelProfiles, reasoningEfforts, updateModelProfile } from '@/lib/model-profiles'
import type { ModelOptions } from '@/lib/model-profiles'

let configRevision = 0
let pendingRequests = 0

/** 配置状态与操作：与后端持久化的应用/模型配置双向同步。 */
export interface ConfigState {
  /** 完整应用配置；未加载时为 null。 */
  config: Config | null
  /** 模型接入配置（config.model 的独立视图，可单独加载/保存）。 */
  modelConfig: ModelConfig | null
  /** 配置的加载/保存是否进行中。 */
  loading: boolean

  /** 从后端加载完整配置，并同步 modelConfig 视图。 */
  loadConfig: () => Promise<void>
  /** 保存完整配置到后端，并同步 modelConfig 视图。 */
  saveConfig: (c: Config) => Promise<void>
  /** 单独从后端加载模型配置。 */
  loadModelConfig: () => Promise<void>
  /** 单独保存模型配置；已加载完整配置时同步其 model 字段。 */
  saveModelConfig: (c: ModelConfig, options?: ModelOptions) => Promise<void>
  /** 用给定配置发一条最小补全请求测试连通性；不改动任何状态。 */
  testConnection: (c: ModelConfig, effort?: ReasoningEffort, thinking?: boolean | null, supportsReasoning?: boolean) => Promise<boolean>
  saveProfiles: (profiles: ModelProfile[], activeId: string) => Promise<void>
  selectModel: (id: string, effort?: ReasoningEffort | null) => Promise<void>
}

export const useConfigStore = create<ConfigState>()((set, get) => ({
  config: null,
  modelConfig: null,
  loading: false,

  loadConfig: async () => {
    const revision = configRevision
    pendingRequests++
    set({ loading: true })
    try {
      const config = await ipc.getConfig()
      if (revision === configRevision) set({ config, modelConfig: config.model })
    } finally {
      pendingRequests--
      set({ loading: pendingRequests > 0 })
    }
  },

  saveConfig: async (c) => {
    configRevision++
    pendingRequests++
    set({ loading: true })
    try {
      await ipc.setConfig(c)
      set({ config: c, modelConfig: c.model })
    } finally {
      pendingRequests--
      set({ loading: pendingRequests > 0 })
    }
  },

  loadModelConfig: async () => {
    const revision = configRevision
    pendingRequests++
    set({ loading: true })
    try {
      const modelConfig = await ipc.getModelConfig()
      // A full config response or model selection may have completed during this request.
      if (revision === configRevision) set((state) => ({ modelConfig: state.config?.model ?? modelConfig }))
    } finally {
      pendingRequests--
      set({ loading: pendingRequests > 0 })
    }
  },

  saveModelConfig: async (c, options) => {
    const config = get().config
    if (config) {
      const selected = activeProfile(config)!
      const next = updateModelProfile({ ...selected, ...options }, c)
      await get().saveProfiles(modelProfiles(config).map((p) => p.id === selected.id ? next : p), selected.id)
      return
    }
    configRevision++
    pendingRequests++
    set({ loading: true })
    try {
      await ipc.setModelConfig(c)
      set({ modelConfig: c })
    } finally {
      pendingRequests--
      set({ loading: pendingRequests > 0 })
    }
  },

  testConnection: (c, effort, thinking, supportsReasoning) => supportsReasoning !== undefined
    ? ipc.testModelConnection(c, effort, thinking ?? undefined, supportsReasoning)
    : thinking != null ? ipc.testModelConnection(c, effort, thinking) : effort ? ipc.testModelConnection(c, effort) : ipc.testModelConnection(c),
  saveProfiles: async (profiles, activeId) => {
    const config = get().config
    const selected = profiles.find((p) => p.id === activeId)
    if (!config || !selected) throw new Error('请选择有效模型')
    await get().saveConfig({ ...config, modelProfiles: profiles, activeModelId: activeId, model: selected.config,
      router: { fastModel: selected.config.name, strongModel: selected.config.name } })
  },
  selectModel: async (id, effort) => {
    const profiles = modelProfiles(get().config)
    const selected = profiles.find((p) => p.id === id)
    if (!selected) throw new Error('模型不存在')
    if (effort != null && !reasoningEfforts(selected).includes(effort)) throw new Error('此模型不支持调节思考强度')
    await get().saveProfiles(profiles.map((p) => p.id === id && effort !== undefined ? { ...p, reasoningEffort: effort } : p), id)
  },
}))
