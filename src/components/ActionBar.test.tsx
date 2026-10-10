import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import ActionBar from './ActionBar'
import { ipc } from '@/services/ipc'
import { useTaskStore } from '@/stores/task'

vi.mock('@/services/ipc', () => ({
  ipc: { applyPatch: vi.fn(), rejectPatch: vi.fn(), rollbackTask: vi.fn(), getFileTree: vi.fn() },
}))
const mockedIpc = vi.mocked(ipc, true)

const task = { id: 1, projectId: 1, prompt: 'fix', status: 'waitingConfirm' as const, intent: null, plan: null, createdAt: 0, finishedAt: null, durationMs: null, tokenInput: 0, tokenOutput: 0, cost: null }
const patch = (id: number, applied = false) => ({ id, taskId: 1, filePath: `${id}.ts`, diff: '', applied, score: null, createdAt: 0 })

describe('ActionBar', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useTaskStore.getState().reset()
    useTaskStore.setState({ currentTask: task, patches: [patch(1), patch(2, true)] })
  })

  it('applies all pending patches and marks them applied', async () => {
    useTaskStore.setState({ patches: [patch(1), patch(3)] })
    mockedIpc.applyPatch.mockResolvedValue(undefined)
    render(<ActionBar />)
    fireEvent.click(screen.getByRole('button', { name: /应用/ }))
    await waitFor(() => expect(mockedIpc.applyPatch).toHaveBeenCalledWith(3))
    expect(useTaskStore.getState().patches.find((p) => p.id === 1)?.applied).toBe(true)
    expect(useTaskStore.getState().patches.find((p) => p.id === 3)?.applied).toBe(true)
    expect(mockedIpc.applyPatch).toHaveBeenCalledTimes(1)
    expect(mockedIpc.getFileTree).toHaveBeenCalledWith(task.projectId)
  })

  it('rejects pending patches and rolls back applied patches', async () => {
    mockedIpc.rejectPatch.mockResolvedValue(undefined)
    mockedIpc.rollbackTask.mockResolvedValue(undefined)
    const { rerender } = render(<ActionBar />)
    fireEvent.click(screen.getByRole('button', { name: /拒绝/ }))
    await waitFor(() => expect(mockedIpc.rejectPatch).toHaveBeenCalledWith(1))
    expect(useTaskStore.getState().patches).toHaveLength(1)
    rerender(<ActionBar />)
    fireEvent.click(screen.getByRole('button', { name: '回滚' }))
    await waitFor(() => expect(mockedIpc.rollbackTask).toHaveBeenCalledWith(1))
    expect(useTaskStore.getState().currentTask?.status).toBe('rolledBack')
  })

  it('shows operation errors', async () => {
    mockedIpc.applyPatch.mockRejectedValue(new Error('conflict'))
    render(<ActionBar />)
    fireEvent.click(screen.getByRole('button', { name: /应用/ }))
    await waitFor(() => expect(screen.getByText('应用失败：conflict')).toBeInTheDocument())
  })
})

