import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import TaskPanel from './TaskPanel'
import { useTaskStore } from '@/stores/task'
import { resetStores, task } from '@/test/fixtures'
import { useConversationStore } from '@/stores/conversation'

beforeEach(resetStores)
afterEach(() => vi.unstubAllGlobals())

it('copies the original input and saved output as plain text, preserving whitespace', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })
  const prompt = '修复函数\n  保留缩进'
  const content = '已完成。\n\n```ts\n  return 0\n```'
  useConversationStore.setState({ selectedId: task.id, record: {
    task: { ...task, prompt }, patches: [], verifies: [], steps: [{
      id: 1, taskId: task.id, stepType: 'event',
      content: JSON.stringify({ type: 'message', taskId: task.id, content }), createdAt: 2500,
    }],
  } })
  render(<TaskPanel />)
  fireEvent.click(screen.getByRole('button', { name: '复制输入' }))
  await waitFor(() => expect(within(screen.getByLabelText('输入信息')).getByText('已复制')).toBeInTheDocument())
  expect(writeText).toHaveBeenNthCalledWith(1, prompt)
  fireEvent.click(screen.getByRole('button', { name: '复制输出' }))
  await waitFor(() => expect(within(screen.getByLabelText('输出信息')).getByText('已复制')).toBeInTheDocument())
  expect(writeText).toHaveBeenNthCalledWith(2, content)
})

it('shows creation and completion times below each message and prefers the saved duration', () => {
  useConversationStore.setState({ selectedId: task.id, record: {
    task: { ...task, durationMs: 65000 }, patches: [], verifies: [], steps: [],
  } })
  render(<TaskPanel />)
  expect(screen.getByLabelText('输入信息').querySelector('time')).toHaveAttribute('datetime', new Date(task.createdAt).toISOString())
  expect(screen.getByLabelText('输出信息').querySelector('time')).toHaveAttribute('datetime', new Date(task.finishedAt!).toISOString())
  expect(within(screen.getByLabelText('输出信息')).getByText('总耗时 1m 05s')).toBeInTheDocument()
})

it('shows total duration immediately on completion, falling back to task timestamps', () => {
  useTaskStore.setState({ currentTask: { ...task, status: 'generating', finishedAt: null, durationMs: null } })
  render(<TaskPanel />)
  expect(screen.queryByText(/总耗时/)).not.toBeInTheDocument()
  const now = Date.now()
  act(() => {
    useTaskStore.getState().setCurrentTask({ ...task, status: 'generating', createdAt: now - 65000, finishedAt: null, durationMs: null })
    useTaskStore.getState().handleEvent({ type: 'done', taskId: task.id, status: 'done' })
  })
  expect(screen.getByText('总耗时 1m 05s')).toBeInTheDocument()
})

it('copies the latest visible streaming output without earlier rounds', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })
  useTaskStore.setState({ currentTask: { ...task, status: 'generating', finishedAt: null, durationMs: null }, events: [
    { type: 'modelText', taskId: task.id, round: 1, delta: 'earlier', status: 'complete' },
    { type: 'modelText', taskId: task.id, round: 2, delta: '当前', status: 'streaming' },
    { type: 'modelText', taskId: task.id, round: 2, delta: '回复', status: 'streaming' },
  ] })
  render(<TaskPanel />)
  fireEvent.click(screen.getByRole('button', { name: '复制输出' }))
  await waitFor(() => expect(writeText).toHaveBeenCalledWith('当前回复'))
  expect(screen.queryByText(/总耗时/)).not.toBeInTheDocument()
})

it('reports clipboard errors and lets the user retry', async () => {
  const writeText = vi.fn().mockRejectedValueOnce(new Error('denied')).mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })
  useTaskStore.setState({ currentTask: task })
  render(<TaskPanel />)
  fireEvent.click(screen.getByRole('button', { name: '复制输入' }))
  expect(await screen.findByText('复制失败，请重试')).toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: '复制输入' }))
  await waitFor(() => expect(screen.getByText('输入已复制')).toBeInTheDocument())
  expect(screen.queryByText('复制失败，请重试')).not.toBeInTheDocument()
})

it('shows incremental text for the current round and retains interrupted output', () => {
  useTaskStore.setState({ currentTask: { ...task, status: 'generating' } })
  render(<TaskPanel />)
  act(() => {
    useTaskStore.getState().handleEvent({ type: 'modelText', taskId: task.id, round: 1, delta: '正在', status: 'streaming' })
  })
  expect(within(screen.getByLabelText('模型回复')).getByText('正在')).toBeInTheDocument()
  act(() => {
    useTaskStore.getState().handleEvent({ type: 'modelText', taskId: task.id, round: 1, delta: '修复。', status: 'interrupted' })
    useTaskStore.getState().handleEvent({ type: 'done', taskId: task.id, status: 'cancelled' })
  })
  expect(screen.getByText('正在修复。')).toBeInTheDocument()
  expect(screen.getByText('回复已中断，内容尚未完成。')).toBeInTheDocument()
})

it('replays persisted deltas without mixing tasks or earlier rounds', () => {
  const events = [
    { type: 'modelText', taskId: 99, round: 2, delta: 'other task', status: 'complete' },
    { type: 'modelText', taskId: task.id, round: 1, delta: 'earlier round', status: 'complete' },
    { type: 'modelText', taskId: task.id, round: 2, delta: '保存的', status: 'streaming' },
    { type: 'modelText', taskId: task.id, round: 2, delta: '回复', status: 'interrupted' },
  ]
  useConversationStore.setState({ selectedId: task.id, record: { task: { ...task, status: 'cancelled' },
    patches: [], verifies: [], steps: events.map((event, id) => ({
      id, taskId: task.id, stepType: 'event', content: JSON.stringify(event), createdAt: id,
    })),
  } })
  render(<TaskPanel />)
  expect(screen.getByText('保存的回复')).toBeInTheDocument()
  expect(screen.queryByText('other task')).not.toBeInTheDocument()
  expect(screen.queryByText('earlier round')).not.toBeInTheDocument()
})

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

it.each([false, true])('shows completion without file changes (historical=%s)', (historical) => {
  const content = '现有文件已满足要求。\n任务已完成，本次无需修改文件';
  const event = { type: 'message' as const, taskId: task.id, content };
  if (historical) {
    useConversationStore.setState({ selectedId: task.id, record: {
      task, patches: [], verifies: [], steps: [{
        id: 1, taskId: task.id, stepType: 'event', content: JSON.stringify(event), createdAt: 2500,
      }],
    } });
  } else {
    useTaskStore.setState({ currentTask: task });
    useTaskStore.getState().handleEvent(event);
  }
  render(<TaskPanel />);
  expect(screen.getByText('任务已完成，本次无需修改文件。')).toBeInTheDocument();
  expect(screen.getByText(/现有文件已满足要求/)).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /^应用/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /撤销|回滚/ })).not.toBeInTheDocument();
  expect(screen.queryByText('任务执行失败。')).not.toBeInTheDocument();
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
