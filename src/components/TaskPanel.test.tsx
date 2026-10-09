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

it('shows unfinished stages as failed after the task fails', () => {
  useTaskStore.setState({ currentTask: { ...task, status: 'failed' } })
  useTaskStore.getState().handleEvent({ type: 'step', taskId: task.id, name: '生成候选', status: 'running' })
  render(<TaskPanel />)
  const steps = screen.getByRole('list', { name: '任务步骤' })
  expect(within(steps).getByText('失败')).toBeInTheDocument()
  expect(within(steps).queryByText('进行中')).not.toBeInTheDocument()
})

it('shows partial changes on failure without offering apply actions', () => {
  useTaskStore.setState({ currentTask: { ...task, status: 'failed' }, patches: [
    { id: 9, taskId: task.id, filePath: 'partial.html', diff: '', applied: false, score: null, createdAt: 0 },
  ] })
  render(<TaskPanel />)
  expect(screen.getByText('partial.html')).toBeInTheDocument()
  expect(screen.queryByRole('button', { name: /应用/ })).not.toBeInTheDocument()
  expect(screen.queryByRole('button', { name: /拒绝/ })).not.toBeInTheDocument()
})

it('shows historical cancelled stages and hides saved plan JSON', () => {
  useConversationStore.setState({ selectedId: task.id, record: {
    task: { ...task, status: 'cancelled' }, patches: [], verifies: [], steps: [
      { id: 1, taskId: task.id, stepType: 'plan', content: '{"steps":[]}', createdAt: 0 },
      { id: 2, taskId: task.id, stepType: '生成候选', content: 'running', createdAt: 0 },
    ],
  } })
  render(<TaskPanel />)
  expect(within(screen.getByRole('list', { name: '任务步骤' })).getByText('已取消')).toBeInTheDocument()
  expect(screen.queryByText('{"steps":[]}')).not.toBeInTheDocument()
})

it('restores progress and final summary from persisted events without showing raw JSON', () => {
  const progress = { type: 'step', taskId: task.id, name: 'read_file #1：src/a.ts', status: 'done' }
  const summary = { type: 'message', taskId: task.id, content: 'Updated authentication.' }
  useConversationStore.setState({ selectedId: task.id, record: { task, patches: [], verifies: [], steps: [progress, summary].map((event, id) => ({
    id, taskId: task.id, stepType: 'event', content: JSON.stringify(event), createdAt: id,
  })) } })
  render(<TaskPanel />)
  expect(screen.getByText('Updated authentication.')).toBeInTheDocument()
  expect(screen.getByText('read_file #1：src/a.ts')).toBeInTheDocument()
  expect(screen.queryByText(JSON.stringify(summary))).not.toBeInTheDocument()
})
