import { beforeEach, describe, expect, it, vi } from 'vitest'

import { invoke } from '@tauri-apps/api/core'
import { ipc } from '@/services/ipc'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
const mockedInvoke = vi.mocked(invoke)

describe('ipc service', () => {
  beforeEach(() => vi.clearAllMocks())

  it('maps project and file commands to Tauri IPC', async () => {
    mockedInvoke.mockResolvedValue(undefined)
    await ipc.pickProjectFolder()
    await ipc.openProject('D:/repo')
    await ipc.closeProject()
    await ipc.listProjects()
    await ipc.getFileTree(3)
    await ipc.readFile(3, 'src/main.ts', 2, 8)
    expect(mockedInvoke.mock.calls).toEqual([
      ['pick_project_folder'],
      ['open_project', { path: 'D:/repo' }],
      ['close_project'],
      ['list_projects'],
      ['get_file_tree', { projectId: 3 }],
      ['read_file', { projectId: 3, path: 'src/main.ts', start: 2, end: 8 }],
    ])
  })

  it('maps task and patch commands with exact argument names', async () => {
    mockedInvoke.mockResolvedValue(undefined)
    await ipc.runTask(1, 'fix bug')
    await ipc.cancelTask(2)
    await ipc.getTask(2)
    await ipc.listTasks(1)
    await ipc.listTaskSteps(2)
    await ipc.applyPatch(4)
    await ipc.rejectPatch(5)
    await ipc.rollbackTask(2)
    await ipc.listPatches(2)
    expect(mockedInvoke.mock.calls).toEqual([
      ['run_task', { projectId: 1, prompt: 'fix bug' }],
      ['cancel_task', { taskId: 2 }],
      ['get_task', { taskId: 2 }],
      ['list_tasks', { projectId: 1 }],
      ['list_task_steps', { taskId: 2 }],
      ['apply_patch', { patchId: 4 }],
      ['reject_patch', { patchId: 5 }],
      ['rollback_task', { taskId: 2 }],
      ['list_patches', { taskId: 2 }],
    ])
  })

  it('maps model and application config commands and preserves rejections', async () => {
    const model = { provider: 'openai', name: 'gpt', apiKey: null, baseUrl: null, temperature: 0.2, maxTokens: null }
    const config = { model, router: { fastModel: 'gpt', strongModel: 'gpt' } }
    mockedInvoke.mockResolvedValue(undefined)
    await ipc.getModelConfig()
    await ipc.setModelConfig(model)
    await ipc.testModelConnection(model)
    await ipc.listModels('openai')
    await ipc.listProviders()
    await ipc.getConfig()
    await ipc.setConfig(config)
    expect(mockedInvoke.mock.calls).toEqual([
      ['get_model_config'],
      ['set_model_config', { config: model }],
      ['test_model_connection', { config: model }],
      ['list_models', { provider: 'openai' }],
      ['list_providers'],
      ['get_config'],
      ['set_config', { config }],
    ])
    const error = new Error('backend unavailable')
    mockedInvoke.mockRejectedValueOnce(error)
    await expect(ipc.listProjects()).rejects.toThrow('backend unavailable')
  })
})

