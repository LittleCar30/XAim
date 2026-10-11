import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ExecutionPermissions from './ExecutionPermissions'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { project, resetStores, task } from '@/test/fixtures'
import type { ExecutionSettings, PermissionRequest } from '@/types'

vi.mock('@/services/ipc', () => ({ ipc: { getExecutionSettings: vi.fn(), setExecutionSettings: vi.fn(), revokeExecutionGrant: vi.fn(), getPendingPermission: vi.fn(), resolveExecutionPermission: vi.fn() } }))
const mocked = vi.mocked(ipc, true)
const request: PermissionRequest = { id: 'request-1', taskId: task.id, projectId: project.id, purpose: '需要安装依赖以继续验证',
  scope: { id: 'scope-1', tool: 'run_command', command: 'npm "ci"', location: '任务工作区 / .', network: '需要联网', impact: '安装依赖，可能运行安装脚本' } }
const settings: ExecutionSettings = { mode: 'ask', grants: [], gitNetwork: { mode: 'system', url: '' } }

beforeEach(() => {
  vi.resetAllMocks(); resetStores()
  useProjectStore.setState({ currentProject: project })
  mocked.getExecutionSettings.mockResolvedValue(settings)
  mocked.getPendingPermission.mockResolvedValue(null)
  mocked.resolveExecutionPermission.mockImplementation(async (taskId, requestId, decision) => {
    useTaskStore.getState().handleEvent({ type: 'permissionResolved', taskId, requestId, decision })
  })
})

describe('execution permissions', () => {
  it('defaults to asking and saves a project mode without creating arbitrary grants', async () => {
    mocked.setExecutionSettings.mockResolvedValue({ ...settings, mode: 'project' })
    render(<ExecutionPermissions running={false} />)
    fireEvent.click(screen.getByRole('button', { name: '项目执行权限' }))
    await waitFor(() => expect(screen.getByLabelText('权限模式')).toBeEnabled())
    expect(screen.getByLabelText('权限模式')).toHaveValue('ask')
    fireEvent.change(screen.getByLabelText('权限模式'), { target: { value: 'project' } })
    await waitFor(() => expect(mocked.setExecutionSettings).toHaveBeenCalledWith(project.id, 'project', { mode: 'system', url: '' }))
    expect(screen.getByText('尚未保存授权。执行时可选择“按项目记住”。')).toBeInTheDocument()
  })
  it('displays the backend operation and responds only to that request', async () => {
    useTaskStore.setState({ currentTask: { ...task, status: 'generating' }, pendingPermission: request })
    render(<ExecutionPermissions running />)
    expect(screen.getByRole('dialog', { name: '执行授权' })).toBeInTheDocument()
    expect(screen.getByText('npm "ci"')).toBeInTheDocument()
    expect(screen.getByText(request.scope.location)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '允许本轮' }))
    await waitFor(() => expect(mocked.resolveExecutionPermission).toHaveBeenCalledWith(task.id, 'request-1', 'once'))
    expect(useTaskStore.getState().pendingPermission).toBeNull()
  })
  it('cancels without approving and keeps a newer request when the old response completes', async () => {
    let finish!: () => void
    mocked.resolveExecutionPermission.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve }))
    useTaskStore.setState({ currentTask: { ...task, status: 'generating' }, pendingPermission: request })
    render(<ExecutionPermissions running />)
    fireEvent.click(screen.getByRole('button', { name: '取消' }))
    expect(mocked.resolveExecutionPermission).toHaveBeenCalledWith(task.id, 'request-1', 'deny')
    act(() => useTaskStore.setState({ pendingPermission: { ...request, id: 'new-request' } }))
    await act(async () => finish())
    expect(useTaskStore.getState().pendingPermission?.id).toBe('new-request')
  })
  it('shows and revokes saved scopes and saves a process-local custom Git proxy', async () => {
    mocked.getExecutionSettings.mockResolvedValue({ ...settings, mode: 'project', grants: [request.scope] })
    mocked.revokeExecutionGrant.mockResolvedValue({ ...settings, mode: 'project' })
    mocked.setExecutionSettings.mockResolvedValue({ ...settings, mode: 'project', gitNetwork: { mode: 'custom', url: 'http://127.0.0.1:7897' } })
    render(<ExecutionPermissions running={false} />)
    fireEvent.click(screen.getByRole('button', { name: '项目执行权限' }))
    fireEvent.click(await screen.findByRole('button', { name: '撤销授权' }))
    await waitFor(() => expect(mocked.revokeExecutionGrant).toHaveBeenCalledWith(project.id, 'scope-1'))
    fireEvent.change(screen.getByLabelText('Git 网络'), { target: { value: 'custom' } })
    fireEvent.change(screen.getByLabelText('代理地址'), { target: { value: 'http://127.0.0.1:7897' } })
    fireEvent.click(screen.getByRole('button', { name: '保存代理' }))
    await waitFor(() => expect(mocked.setExecutionSettings).toHaveBeenCalledWith(project.id, 'project', { mode: 'custom', url: 'http://127.0.0.1:7897' }))
  })
})
