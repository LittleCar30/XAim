import { act, render, screen, within } from '@testing-library/react'
import { beforeEach, expect, it } from 'vitest'

import TaskPanel from './TaskPanel'
import { useTaskStore } from '@/stores/task'
import { resetStores, task } from '@/test/fixtures'
import { useConversationStore } from '@/stores/conversation'

beforeEach(resetStores)

it('renders empty state then responds to task events', () => {
  render(<TaskPanel />)
  expect(screen.getByText('新对话')).toBeInTheDocument()
  act(() => {
    useTaskStore.getState().setCurrentTask({ ...task, status: 'verifying' })
    useTaskStore.getState().handleEvent({ type: 'step', taskId: task.id, name: '运行测试', status: 'running' })
    useTaskStore.getState().handleEvent({ type: 'verify', taskId: task.id, kind: 'test', passed: true, output: 'tests passed' })
  })
  expect(screen.getByText('验证中')).toBeInTheDocument()
  expect(screen.getByText('fix login')).toBeInTheDocument()
  expect(within(screen.getByRole('list', { name: '任务步骤' })).getByText('运行测试')).toBeInTheDocument()
  expect(screen.getByText('验证 · 1/1 通过')).toBeInTheDocument()
})

it('shows historical messages without offering live patch actions', () => {
  useTaskStore.setState({ currentTask: { ...task, prompt: 'active task' } })
  useConversationStore.setState({ selectedId: 99, record: {
    task: { ...task, id: 99, prompt: 'old task' }, steps: [], verifies: [],
    patches: [{ id: 9, taskId: 99, filePath: 'src/old.ts', diff: '', applied: true, score: null, createdAt: 0 }],
  } })
  render(<TaskPanel />)
  expect(screen.getByText('old task')).toBeInTheDocument()
  expect(screen.queryByText('active task')).not.toBeInTheDocument()
  expect(screen.queryByRole('button', { name: '回滚' })).not.toBeInTheDocument()
})
