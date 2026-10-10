import { beforeEach, expect, it, vi } from 'vitest'
import { ipc } from '@/services/ipc'
import { useConversationStore } from './conversation'
import { useTaskStore } from './task'
import { patch, resetStores, task } from '@/test/fixtures'
import type { Task } from '@/types'

vi.mock('@/services/ipc', () => ({ ipc: { getTask: vi.fn(), listTaskSteps: vi.fn(), listPatches: vi.fn(), listConversationTasks: vi.fn() } }))
const mocked = vi.mocked(ipc, true)
beforeEach(() => { vi.clearAllMocks(); resetStores(); mocked.listConversationTasks.mockResolvedValue([]); mocked.listTaskSteps.mockResolvedValue([]); mocked.listPatches.mockResolvedValue([patch]) })

it('opens a persisted task without replacing the active task or its patches', async () => {
  const live = { ...task, id: 90, status: 'generating' as const }
  useTaskStore.setState({ currentTask: live, patches: [] })
  mocked.getTask.mockResolvedValue(task)
  await useConversationStore.getState().select(task.id)
  expect(mocked.listTaskSteps).toHaveBeenCalledWith(task.id)
  expect(useConversationStore.getState().record?.patches).toEqual([patch])
  expect(useTaskStore.getState().currentTask).toEqual(live)
  expect(useTaskStore.getState().patches).toEqual([])
})

it('ignores a delayed history response after returning to the active conversation', async () => {
  let resolve!: (task: Task) => void
  mocked.getTask.mockReturnValue(new Promise((done) => { resolve = done }))
  const pending = useConversationStore.getState().select(task.id)
  await useConversationStore.getState().select(null)
  resolve(task)
  await pending
  expect(useConversationStore.getState().selectedId).toBeNull()
  expect(useConversationStore.getState().record).toBeNull()
})

it('archives verification results for tasks visited later in the same session', async () => {
  const verifies = [{ kind: 'test', passed: true, output: 'ok', durationMs: 10 }]
  useTaskStore.setState({ currentTask: task, verifies })
  useConversationStore.getState().archiveCurrent()
  useTaskStore.getState().reset()
  mocked.getTask.mockResolvedValue(task)
  await useConversationStore.getState().select(task.id)
  expect(useConversationStore.getState().record?.verifies).toEqual(verifies)
})

it('retains history selection when navigation remounts but clears it when the project changes', () => {
  useConversationStore.getState().syncProject(1)
  useConversationStore.setState({ selectedId: task.id })
  useConversationStore.getState().syncProject(1)
  expect(useConversationStore.getState().selectedId).toBe(task.id)
  useConversationStore.getState().syncProject(2)
  expect(useConversationStore.getState().selectedId).toBeNull()
})

it('restores verification output from persisted events after session cache is cleared', async () => {
  mocked.getTask.mockResolvedValue(task)
  mocked.listTaskSteps.mockResolvedValue([{ id: 1, taskId: task.id, stepType: 'event', createdAt: 1,
    content: JSON.stringify({ type: 'verify', taskId: task.id, kind: 'test', passed: false, status: 'unavailable', output: 'missing runner' }) }])
  await useConversationStore.getState().select(task.id)
  expect(useConversationStore.getState().record?.verifies).toEqual([{ kind: 'test', passed: false, status: 'unavailable', output: 'missing runner', durationMs: 0 }])
})
