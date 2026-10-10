import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ModelPicker from './ModelPicker'
import Settings from '@/pages/Settings'
import { ipc } from '@/services/ipc'
import { useConfigStore } from '@/stores/config'
import type { Config } from '@/types'

vi.mock('@/services/ipc', () => ({ ipc: {
  getConfig: vi.fn(), setConfig: vi.fn(), listModels: vi.fn(), listProviders: vi.fn(), testModelConnection: vi.fn(),
  setPreference: vi.fn(), getStorageInfo: vi.fn(), getRuntimeSettings: vi.fn(),
} }))
const mocked = vi.mocked(ipc, true)
let persisted: Config

describe('model settings across both entry points', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    localStorage.clear()
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, value() { this.setAttribute('open', '') } })
    const model = { provider: 'newapi', name: 'custom-reasoner', apiKey: 'test-key', baseUrl: 'https://example.test/v1', temperature: 0.7, maxTokens: null }
    persisted = { model, router: { fastModel: model.name, strongModel: model.name }, activeModelId: 'gateway',
      modelProfiles: [{ id: 'gateway', label: 'Gateway', config: model, supportsReasoning: true, reasoningEffort: 'high', thinking: null }] }
    mocked.getConfig.mockImplementation(async () => structuredClone(persisted))
    mocked.setConfig.mockImplementation(async (next) => { persisted = structuredClone(next) })
    mocked.listModels.mockResolvedValue([])
    mocked.listProviders.mockResolvedValue(['openai', 'newapi', 'deepseek', 'qwen'])
    mocked.testModelConnection.mockResolvedValue(true)
    mocked.setPreference.mockResolvedValue(undefined)
    mocked.getStorageInfo.mockRejectedValue(new Error('storage not available in this test'))
    mocked.getRuntimeSettings.mockResolvedValue({ defaults: {}, shared: {}, local: {}, effective: { taskTimeoutSec: 1800, maxTurns: 32 } })
    useConfigStore.setState({ config: null, modelConfig: null, loading: false })
  })

  it('shares persisted parameters between composer management, Settings and subsequent loads', async () => {
    render(<><ModelPicker /><Settings /></>)
    await waitFor(() => expect(screen.getByRole('button', { name: '保存全部' })).toBeEnabled())
    expect(screen.getByLabelText('Temperature')).toBeDisabled()
    expect(screen.getByLabelText('思考强度')).toHaveValue('high')
    fireEvent.click(screen.getByRole('button', { name: '选择模型' }))
    fireEvent.click(screen.getByRole('menuitem', { name: '管理模型' }))
    const dialog = within(screen.getByRole('dialog'))
    fireEvent.change(dialog.getByLabelText('思考强度'), { target: { value: 'default' } })
    fireEvent.change(dialog.getByLabelText('Max Tokens'), { target: { value: '16384' } })
    fireEvent.click(dialog.getByRole('button', { name: '保存模型' }))
    await dialog.findByText('已保存')
    fireEvent.click(dialog.getByRole('button', { name: '关闭模型管理' }))
    expect(screen.getByLabelText('思考强度')).toHaveValue('default')
    expect(screen.getByLabelText('Max Tokens')).toHaveValue(16384)
    fireEvent.click(screen.getByRole('button', { name: '测试连接' }))
    await screen.findByText('连接成功')
    expect(mocked.testModelConnection).toHaveBeenLastCalledWith(persisted.model, undefined, undefined, true)
    fireEvent.change(screen.getByLabelText('思考强度'), { target: { value: 'medium' } })
    fireEvent.change(screen.getByLabelText('Max Tokens'), { target: { value: '' } })
    fireEvent.click(screen.getByRole('button', { name: '保存全部' }))
    await screen.findByText('已保存')
    expect(persisted.model.maxTokens).toBeNull()
    expect(persisted.modelProfiles![0]).toMatchObject({ supportsReasoning: true, reasoningEffort: 'medium', config: persisted.model })
    expect(screen.getByRole('button', { name: '选择思考强度' })).toHaveTextContent('中')
    await act(async () => { await useConfigStore.getState().loadConfig() })
    expect(screen.getByLabelText('思考强度')).toHaveValue('medium')
    fireEvent.click(screen.getByRole('button', { name: '管理模型' }))
    expect(within(screen.getByRole('dialog')).getByLabelText('Max Tokens')).toHaveValue(null)
    expect(within(screen.getByRole('dialog')).getByLabelText('思考强度')).toHaveValue('medium')
  })

  it('keeps the previous persisted configuration when saving fails', async () => {
    render(<Settings />)
    await waitFor(() => expect(screen.getByRole('button', { name: '保存全部' })).toBeEnabled())
    const previous = structuredClone(persisted)
    mocked.setConfig.mockRejectedValueOnce(new Error('disk full'))
    fireEvent.change(screen.getByLabelText('Max Tokens'), { target: { value: '9000' } })
    fireEvent.click(screen.getByRole('button', { name: '保存全部' }))
    await screen.findByText(/模型配置保存失败.*disk full/)
    expect(persisted).toEqual(previous)
    expect(useConfigStore.getState().config).toEqual(previous)
  })

  it('preserves unsaved model edits when switching settings categories', async () => {
    render(<Settings />)
    await waitFor(() => expect(screen.getByRole('button', { name: '保存全部' })).toBeEnabled())
    fireEvent.change(screen.getByLabelText('Max Tokens'), { target: { value: '9000' } })
    fireEvent.click(screen.getByRole('button', { name: '界面与外观' }))
    expect(screen.getByLabelText('主题')).toBeVisible()
    expect(screen.getByLabelText('Max Tokens')).not.toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: '模型与连接' }))
    expect(screen.getByLabelText('Max Tokens')).toBeVisible()
    expect(screen.getByLabelText('Max Tokens')).toHaveValue(9000)
    expect(mocked.setConfig).not.toHaveBeenCalled()
  })

  it('opens the category containing a validation error when saving from another category', async () => {
    render(<Settings />)
    await waitFor(() => expect(screen.getByRole('button', { name: '保存全部' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: '项目索引' }))
    fireEvent.change(screen.getByLabelText('最大文件数'), { target: { value: '0' } })
    fireEvent.click(screen.getByRole('button', { name: '界面与外观' }))
    fireEvent.click(screen.getByRole('button', { name: '保存全部' }))
    expect(screen.getByRole('button', { name: '项目索引' })).toHaveAttribute('aria-current', 'page')
    expect(screen.getByText('取值范围 1 ~ 1000000')).toBeVisible()
    expect(mocked.setConfig).not.toHaveBeenCalled()
  })

  it('reports preference persistence failure without claiming model settings were saved', async () => {
    render(<Settings />)
    await waitFor(() => expect(screen.getByRole('button', { name: '保存全部' })).toBeEnabled())
    mocked.setPreference.mockRejectedValueOnce(new Error('database unavailable'))
    fireEvent.click(screen.getByRole('button', { name: '保存全部' }))
    await screen.findByText(/偏好设置保存失败.*database unavailable/)
    expect(mocked.setConfig).not.toHaveBeenCalled()
    expect(screen.queryByText('已保存')).not.toBeInTheDocument()
  })

  it('saves draft edits when activating another model instead of using old settings', async () => {
    const second = { ...persisted.modelProfiles![0], id: 'second', label: 'Second', config: { ...persisted.model, name: 'second-model' } }
    persisted.modelProfiles!.push(second)
    render(<><ModelPicker /><Settings /></>)
    await waitFor(() => expect(screen.getByRole('button', { name: '保存全部' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: '管理模型' }))
    const dialog = within(screen.getByRole('dialog'))
    fireEvent.click(dialog.getByRole('button', { name: /Second/ }))
    fireEvent.change(dialog.getByLabelText('Max Tokens'), { target: { value: '12000' } })
    fireEvent.click(dialog.getByRole('button', { name: '设为当前' }))
    await dialog.findByText('已保存并设为当前模型')
    expect(persisted.activeModelId).toBe('second')
    expect(persisted.model.maxTokens).toBe(12000)
    expect(persisted.router.fastModel).toBe('second-model')
    fireEvent.click(dialog.getByRole('button', { name: '关闭模型管理' }))
    expect(screen.getByLabelText('Max Tokens')).toHaveValue(12000)
    expect(screen.getByRole('button', { name: '选择模型' })).toHaveTextContent('Second')
  })

  it('uses configured temperature when thinking is explicitly disabled', async () => {
    const model = { ...persisted.model, provider: 'deepseek', name: 'deepseek-flash', temperature: 0.25 }
    persisted = { ...persisted, model, modelProfiles: [{ ...persisted.modelProfiles![0], config: model, supportsReasoning: false, reasoningEffort: null, thinking: false }] }
    render(<Settings />)
    await waitFor(() => expect(screen.getByRole('button', { name: '保存全部' })).toBeEnabled())
    expect(screen.getByLabelText('Temperature')).toBeEnabled()
    fireEvent.change(screen.getByLabelText('Temperature'), { target: { value: '0.5' } })
    fireEvent.click(screen.getByRole('button', { name: '测试连接' }))
    await screen.findByText('连接成功')
    expect(mocked.testModelConnection).toHaveBeenLastCalledWith(expect.objectContaining({ temperature: 0.5 }), undefined, false, false)
  })
})
