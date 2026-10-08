import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'

import HistoryList from './HistoryList'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { project, resetStores, task } from '@/test/fixtures'

vi.mock('@/services/ipc', () => ({ ipc: { listTasks: vi.fn(), rollbackTask: vi.fn() } }))
const mocked = vi.mocked(ipc, true)

beforeEach(() => { vi.resetAllMocks(); resetStores() })

it('shows no-project state and disables refresh', () => {
  render(<HistoryList />)
  expect(screen.getByText('未打开项目')).toBeInTheDocument()
  expect(screen.getByRole('button', { name: '刷新历史任务' })).toBeDisabled()
})

it('loads history, expands task statistics and rolls back task', async () => {
  useProjectStore.setState({ currentProject: project })
  mocked.listTasks.mockResolvedValue([task])
  mocked.rollbackTask.mockResolvedValue(undefined)
  render(<HistoryList />)
  fireEvent.click(await screen.findByRole('button', { name: /fix login/ }))
  expect(screen.getByText('repair auth')).toBeInTheDocument()
  expect(screen.getByText('Token')).toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: '回滚此任务的变更' }))
  await waitFor(() => expect(useTaskStore.getState().history[0].status).toBe('rolledBack'))
  expect(mocked.rollbackTask).toHaveBeenCalledWith(42)
  expect(screen.queryByRole('button', { name: '回滚此任务的变更' })).not.toBeInTheDocument()
})

it('refreshes after terminal event and shows history load errors', async () => {
  useProjectStore.setState({ currentProject: project })
  mocked.listTasks.mockResolvedValue([])
  render(<HistoryList />)
  expect(await screen.findByText('暂无历史任务')).toBeInTheDocument()
  mocked.listTasks.mockRejectedValueOnce('database unavailable')
  act(() => useTaskStore.setState({ currentTask: task }))
  expect(await screen.findByText('database unavailable')).toBeInTheDocument()
})
