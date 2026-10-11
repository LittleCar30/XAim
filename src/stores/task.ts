import { create } from 'zustand'

import { ipc } from '@/services/ipc'
import { toolContext } from '@/lib/tool-context'
import type {
  ContextFile,
  Patch,
  Task,
  TaskStatus,
  TaskStep,
  ToolLog,
  ToolResult,
  VerifyResult,
  XaimEvent,
  PermissionRequest,
} from '@/types'

/** 当前任务的状态与操作：由 ipc 与后端事件流共同驱动。 */
export interface TaskState {
  pendingPermission: PermissionRequest | null
  /** 当前任务；未运行任务时为 null。 */
  currentTask: Task | null
  /** 当前任务的步骤流水。 */
  steps: TaskStep[]
  /** 当前任务产出的候选补丁。 */
  patches: Patch[]
  /** 当前任务的验证结果。 */
  verifies: VerifyResult[]
  /** 进入模型上下文的文件片段。 */
  context: ContextFile[]
  /** 工具调用日志。 */
  toolLogs: ToolLog[]
  /** 收到的原始事件流（全类型留档，供时间线/调试展示）。 */
  events: XaimEvent[]
  /** 当前项目的历史任务（最新在前）。 */
  history: Task[]

  setCurrentTask: (t: Task) => void
  addStep: (s: TaskStep) => void
  addPatch: (p: Patch) => void
  addVerify: (v: VerifyResult) => void
  addToolLog: (l: ToolLog) => void
  /** 镜像 apply_patch / rollback_task 命令结果：更新候选补丁的应用标记。 */
  setPatchApplied: (patchId: number, applied: boolean) => void
  /** 镜像 reject_patch 命令结果：被拒绝的补丁不再是候选，从列表移除。 */
  removePatch: (patchId: number) => void
  /** 写入检索阶段组装的模型上下文（后端 Context；待事件/命令桥接后由数据层填充）。 */
  setContext: (files: ContextFile[]) => void
  /** 镜像工具执行结果（后端工具事件暂不携带结果，待结果通道落地后调用）。 */
  setToolResult: (entryIndex: number, result: ToolResult) => void
  /** 覆写历史列表（测试/清空用）。 */
  setHistory: (tasks: Task[]) => void
  /** 拉取指定项目的历史任务（list_tasks），失败时抛出由调用方展示。 */
  loadHistory: (projectId: number) => Promise<void>
  /** 镜像 rollback_task 命令结果：历史中对应任务标记为已回滚。 */
  markTaskRolledBack: (taskId: number) => void
  /**
   * 按事件类型分发更新：始终追加到 events；step/tool/patch/verify 追加对应列表；
   * token 累加 currentTask 用量；done 推进 currentTask 终态；
   * repair/error 无专用字段，仅保留在 events 中。
   */
  handleEvent: (e: XaimEvent) => void
  /** 清空全部任务状态（关闭项目或切换任务时调用）。 */
  reset: () => void
}

export const useTaskStore = create<TaskState>()((set, get) => ({
  pendingPermission: null,
  currentTask: null,
  steps: [],
  patches: [],
  verifies: [],
  context: [],
  toolLogs: [],
  events: [],
  history: [],

  setCurrentTask: (t) => set({ currentTask: t }),

  addStep: (s) => set((state) => ({ steps: [...state.steps, s] })),

  addPatch: (p) => set((state) => ({ patches: [...state.patches, p] })),

  addVerify: (v) => set((state) => ({ verifies: [...state.verifies, v] })),

  addToolLog: (l) => set((state) => ({ toolLogs: [...state.toolLogs, l] })),

  setPatchApplied: (patchId, applied) =>
    set((state) => ({
      patches: state.patches.map((p) => (p.id === patchId ? { ...p, applied } : p)),
    })),

  removePatch: (patchId) =>
    set((state) => ({ patches: state.patches.filter((p) => p.id !== patchId) })),

  setContext: (files) => set({ context: files }),

  setToolResult: (entryIndex, result) =>
    set((state) => ({
      toolLogs: state.toolLogs.map((log, i) => (i === entryIndex ? { ...log, result } : log)),
    })),

  setHistory: (tasks) => set({ history: tasks }),

  loadHistory: async (projectId) => {
    set({ history: await ipc.listTasks(projectId) })
  },

  markTaskRolledBack: (taskId) =>
    set((state) => ({
      history: state.history.map((t) =>
        t.id === taskId ? { ...t, status: 'rolledBack' as const } : t,
      ),
    })),

  handleEvent: (event) => {
    // The first backend event resolves the optimistic message's task identity.
    const pending = get().currentTask
    if (pending?.id === -1) set({ currentTask: { ...pending, id: event.taskId } })
    set((state) => ({ events: [...state.events, event] }))

    switch (event.type) {
      case 'permissionRequested':
        if (get().currentTask?.id === event.taskId) set({ pendingPermission: event.request })
        break
      case 'permissionResolved':
        if (get().pendingPermission?.id === event.requestId) set({ pendingPermission: null })
        break
      case 'step':
        // 事件流不含步骤行 id，以列表内位置代替；stepType=步骤名，content=状态。
        get().addStep({
          id: get().steps.length,
          taskId: event.taskId,
          stepType: event.name,
          content: event.status,
          createdAt: Date.now(),
        })
        break
      case 'tool':
        get().addToolLog({
          taskId: event.taskId,
          tool: event.tool,
          args: event.args,
          createdAt: Date.now(),
        })
        break
      case 'toolFinished': {
        const index = get().toolLogs.findIndex((log) => {
          try { return JSON.parse(log.args).call_id === event.callId } catch { return false }
        })
        if (index >= 0) get().setToolResult(index, event.result)
        const files = toolContext(event.result)
        if (files.length > 0) {
          set((state) => {
            const context = new Map(state.context.map((file) => [file.path, file]))
            for (const file of files) context.set(file.path, file)
            return { context: [...context.values()] }
          })
        }
        break
      }
      case 'message':
        break
      case 'patch':
        get().addPatch({
          id: get().patches.length,
          taskId: event.taskId,
          filePath: event.file,
          diff: event.diff,
          applied: false,
          score: null,
          createdAt: Date.now(),
        })
        break
      case 'verify':
        // 事件不含耗时，先以 0 占位。
        get().addVerify({
          kind: event.kind,
          passed: event.passed,
          output: event.output,
          ...(event.status ? { status: event.status } : {}),
          durationMs: 0,
        })
        break
      case 'token': {
        const { currentTask } = get()
        if (!currentTask || currentTask.id !== event.taskId) break
        set({
          currentTask: {
            ...currentTask,
            tokenInput: (currentTask.tokenInput ?? 0) + event.input,
            tokenOutput: (currentTask.tokenOutput ?? 0) + event.output,
          },
        })
        break
      }
      case 'done': {
        // status 为后端 TaskStatus 的 camelCase 序列化值，与前端联合类型一致。
        const { currentTask } = get()
        if (!currentTask || currentTask.id !== event.taskId) break
        set({
          pendingPermission: null,
          currentTask: {
            ...currentTask,
            status: event.status as TaskStatus,
            finishedAt: Date.now(),
          },
        })
        break
      }
      case 'repair':
      case 'error':
        if (event.type === 'error' && get().currentTask?.id === event.taskId) {
          set({ pendingPermission: null })
          const blocked = event.message.startsWith('任务受阻:')
          set({ currentTask: { ...get().currentTask!, status: blocked ? 'blocked' : 'failed', finishedAt: Date.now() } })
        }
        break
    }
  },

  reset: () =>
    set({
      pendingPermission: null,
      currentTask: null,
      steps: [],
      patches: [],
      verifies: [],
      context: [],
      toolLogs: [],
      events: [],
      history: [],
    }),
}))
