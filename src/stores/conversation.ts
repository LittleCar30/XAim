import { create } from 'zustand'
import { ipc } from '@/services/ipc'
import { useTaskStore } from '@/stores/task'
import type { Patch, Task, TaskStep, VerifyResult, XaimEvent } from '@/types'

export interface ConversationRecord {
  task: Task
  steps: TaskStep[]
  patches: Patch[]
  verifies: VerifyResult[]
}

let requestVersion = 0
export const useConversationStore = create<{
  projectId: number | null
  selectedId: number | null
  record: ConversationRecord | null
  loading: boolean
  error: string | null
  cache: Record<number, ConversationRecord>
  turns: ConversationRecord[]
  loadTurns: (taskId: number) => Promise<void>
  archiveCurrent: () => void
  select: (taskId: number | null) => Promise<void>
  clear: () => void
  syncProject: (projectId: number | null) => void
}>()((set, get) => ({
  projectId: null,
  selectedId: null, record: null, loading: false, error: null, cache: {}, turns: [],
  loadTurns: async (taskId) => {
    const version = requestVersion
    const tasks = await ipc.listConversationTasks(taskId)
    const turns = await Promise.all(tasks.map(async (task): Promise<ConversationRecord> => {
      const [steps, patches] = await Promise.all([ipc.listTaskSteps(task.id), ipc.listPatches(task.id)])
      const verifies = steps.filter((s) => s.stepType === 'event').flatMap((s): VerifyResult[] => {
        try {
          const e = JSON.parse(s.content) as XaimEvent
          return e.type === 'verify' ? [{ kind: e.kind, passed: e.passed, output: e.output, durationMs: 0, ...(e.status ? { status: e.status } : {}) }] : []
        } catch { return [] }
      })
      return { task, steps, patches, verifies }
    }))
    if (version === requestVersion) set({ turns })
  },
  archiveCurrent: () => {
    const { currentTask, steps, patches, verifies } = useTaskStore.getState()
    if (!currentTask || currentTask.id < 0) return
    set({ cache: { ...get().cache, [currentTask.id]: { task: currentTask, steps, patches, verifies } } })
  },
  select: async (taskId) => {
    const version = ++requestVersion
    if (taskId === null || taskId === useTaskStore.getState().currentTask?.id) {
      set({ selectedId: null, record: null, loading: false, error: null })
      if (taskId !== null) await get().loadTurns(taskId).catch((error) => {
        if (version === requestVersion) set({ error: error instanceof Error ? error.message : String(error) })
      })
      return
    }
    set({ selectedId: taskId, record: null, loading: true, error: null })
    try {
      const [task, steps, patches] = await Promise.all([
        ipc.getTask(taskId), ipc.listTaskSteps(taskId), ipc.listPatches(taskId),
      ])
      if (version !== requestVersion) return
      const persisted = steps.filter((step) => step.stepType === 'event').flatMap((step): VerifyResult[] => {
        try {
          const event = JSON.parse(step.content) as XaimEvent
          return event.type === 'verify' ? [{ kind: event.kind, passed: event.passed, output: event.output, durationMs: 0,
            ...(event.status ? { status: event.status } : {}) }] : []
        } catch { return [] }
      })
      set({ record: { task, steps, patches, verifies: persisted.length ? persisted : get().cache[taskId]?.verifies ?? [] }, loading: false })
      await get().loadTurns(taskId)
    } catch (error) {
      if (version !== requestVersion) return
      set({ loading: false, error: error instanceof Error ? error.message : String(error) })
    }
  },
  clear: () => {
    requestVersion++
    set({ selectedId: null, record: null, loading: false, error: null, cache: {}, turns: [] })
  },
  syncProject: (projectId) => {
    if (get().projectId === projectId) return
    get().clear()
    set({ projectId })
  },
}))
