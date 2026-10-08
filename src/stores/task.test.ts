import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ipc } from '@/services/ipc'
import { useTaskStore } from '@/stores/task'
import type { Patch, Task, TaskStep, ToolLog, VerifyResult, XaimEvent } from '@/types'

vi.mock('@/services/ipc', () => ({
  ipc: {
    listTasks: vi.fn(),
  },
}))

const mockedIpc = vi.mocked(ipc, true)

const NOW = 1_700_000_000_000

const task: Task = {
  id: 1,
  projectId: 1,
  prompt: '修复空指针',
  status: 'generating',
  intent: '修复空指针',
  plan: null,
  createdAt: NOW - 1000,
  finishedAt: null,
  durationMs: null,
  tokenInput: 10,
  tokenOutput: 5,
  cost: null,
}

describe('useTaskStore', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.useFakeTimers()
    vi.setSystemTime(NOW)
    useTaskStore.getState().reset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('has empty initial state', () => {
    const state = useTaskStore.getState()
    expect(state.currentTask).toBeNull()
    expect(state.steps).toEqual([])
    expect(state.patches).toEqual([])
    expect(state.verifies).toEqual([])
    expect(state.context).toEqual([])
    expect(state.toolLogs).toEqual([])
    expect(state.events).toEqual([])
  })

  it('setCurrentTask stores the task', () => {
    useTaskStore.getState().setCurrentTask(task)
    expect(useTaskStore.getState().currentTask).toEqual(task)
  })

  it('add* appends to the matching list', () => {
    const { addStep, addPatch, addVerify, addToolLog } = useTaskStore.getState()

    const step: TaskStep = { id: 0, taskId: 1, stepType: '解析意图', content: 'done', createdAt: NOW }
    const patch: Patch = {
      id: 0,
      taskId: 1,
      filePath: 'src/main.rs',
      diff: '--- a +++ b',
      applied: false,
      score: null,
      createdAt: NOW,
    }
    const verify: VerifyResult = { kind: 'cargo_test', passed: true, output: 'ok', durationMs: 5 }
    const log: ToolLog = { taskId: 1, tool: 'read_file', args: '{"path":"a"}', createdAt: NOW }

    addStep(step)
    addPatch(patch)
    addVerify(verify)
    addToolLog(log)

    const state = useTaskStore.getState()
    expect(state.steps).toEqual([step])
    expect(state.patches).toEqual([patch])
    expect(state.verifies).toEqual([verify])
    expect(state.toolLogs).toEqual([log])
  })

  it('handleEvent always appends the raw event', () => {
    const event: XaimEvent = { type: 'repair', taskId: 1, round: 2 }
    useTaskStore.getState().handleEvent(event)
    expect(useTaskStore.getState().events).toEqual([event])
  })

  it('handleEvent step maps name/status into a TaskStep', () => {
    useTaskStore.getState().handleEvent({
      type: 'step',
      taskId: 1,
      name: '生成补丁',
      status: 'running',
    })

    const { steps, events } = useTaskStore.getState()
    expect(steps).toEqual([
      { id: 0, taskId: 1, stepType: '生成补丁', content: 'running', createdAt: NOW },
    ])
    expect(events).toEqual([{ type: 'step', taskId: 1, name: '生成补丁', status: 'running' }])
  })

  it('handleEvent tool appends a ToolLog', () => {
    useTaskStore
      .getState()
      .handleEvent({ type: 'tool', taskId: 1, tool: 'grep', args: '{"q":"foo"}' })

    expect(useTaskStore.getState().toolLogs).toEqual([
      { taskId: 1, tool: 'grep', args: '{"q":"foo"}', createdAt: NOW },
    ])
  })

  it('handleEvent patch maps file/diff into an unapplied Patch', () => {
    useTaskStore
      .getState()
      .handleEvent({ type: 'patch', taskId: 1, file: 'src/lib.rs', diff: '+++ b' })

    const { patches } = useTaskStore.getState()
    expect(patches).toEqual([
      {
        id: 0,
        taskId: 1,
        filePath: 'src/lib.rs',
        diff: '+++ b',
        applied: false,
        score: null,
        createdAt: NOW,
      },
    ])
  })

  it('handleEvent verify appends a VerifyResult with duration 0', () => {
    useTaskStore
      .getState()
      .handleEvent({ type: 'verify', taskId: 1, kind: 'cargo_test', passed: false, output: 'fail' })

    expect(useTaskStore.getState().verifies).toEqual([
      { kind: 'cargo_test', passed: false, output: 'fail', durationMs: 0 },
    ])
  })

  it('handleEvent token accumulates usage into the current task', () => {
    useTaskStore.getState().setCurrentTask(task)
    useTaskStore.getState().handleEvent({ type: 'token', taskId: 1, input: 3, output: 2 })
    useTaskStore.getState().handleEvent({ type: 'token', taskId: 1, input: 1, output: 1 })

    const current = useTaskStore.getState().currentTask
    expect(current?.tokenInput).toBe(14)
    expect(current?.tokenOutput).toBe(8)
  })

  it('handleEvent token ignores events of other tasks', () => {
    useTaskStore.getState().setCurrentTask(task)
    useTaskStore.getState().handleEvent({ type: 'token', taskId: 99, input: 100, output: 100 })

    const current = useTaskStore.getState().currentTask
    expect(current?.tokenInput).toBe(10)
    expect(current?.tokenOutput).toBe(5)
  })

  it('handleEvent done advances the current task to its terminal status', () => {
    useTaskStore.getState().setCurrentTask(task)
    useTaskStore.getState().handleEvent({ type: 'done', taskId: 1, status: 'done' })

    const current = useTaskStore.getState().currentTask
    expect(current?.status).toBe('done')
    expect(current?.finishedAt).toBe(NOW)
  })

  it('handleEvent done ignores other tasks and unknown task ids', () => {
    useTaskStore.getState().handleEvent({ type: 'done', taskId: 1, status: 'failed' })
    expect(useTaskStore.getState().currentTask).toBeNull()

    useTaskStore.getState().setCurrentTask(task)
    useTaskStore.getState().handleEvent({ type: 'done', taskId: 99, status: 'failed' })
    expect(useTaskStore.getState().currentTask?.status).toBe('generating')
  })

  it('handleEvent repair and error only archive into events', () => {
    const { handleEvent } = useTaskStore.getState()
    handleEvent({ type: 'repair', taskId: 1, round: 1 })
    handleEvent({ type: 'error', taskId: 1, message: 'boom' })

    const state = useTaskStore.getState()
    expect(state.events).toHaveLength(2)
    expect(state.steps).toEqual([])
    expect(state.patches).toEqual([])
    expect(state.verifies).toEqual([])
    expect(state.toolLogs).toEqual([])
  })

  it('setPatchApplied flips the applied flag of one patch', () => {
    const { addPatch, setPatchApplied } = useTaskStore.getState()
    addPatch({ id: 1, taskId: 1, filePath: 'a.ts', diff: '', applied: false, score: null, createdAt: NOW })
    addPatch({ id: 2, taskId: 1, filePath: 'b.ts', diff: '', applied: false, score: null, createdAt: NOW })

    setPatchApplied(1, true)

    const patches = useTaskStore.getState().patches
    expect(patches.find((p) => p.id === 1)?.applied).toBe(true)
    expect(patches.find((p) => p.id === 2)?.applied).toBe(false)
  })

  it('removePatch drops only the rejected patch', () => {
    const { addPatch, removePatch } = useTaskStore.getState()
    addPatch({ id: 1, taskId: 1, filePath: 'a.ts', diff: '', applied: false, score: null, createdAt: NOW })
    addPatch({ id: 2, taskId: 1, filePath: 'b.ts', diff: '', applied: false, score: null, createdAt: NOW })

    removePatch(1)

    const patches = useTaskStore.getState().patches
    expect(patches).toHaveLength(1)
    expect(patches[0].id).toBe(2)
  })

  it('setContext replaces the context file list', () => {
    const { setContext } = useTaskStore.getState()
    const files = [
      { path: 'src/lib.rs', content: 'fn main() {}', reason: '入口文件', tokens: 42 },
      { path: 'src/diff.rs', content: 'pub struct Hunk', reason: 'grep 命中', tokens: 18 },
    ]

    setContext(files)

    expect(useTaskStore.getState().context).toEqual(files)
  })

  it('setToolResult attaches a result to one entry without touching others', () => {
    const { addToolLog, setToolResult } = useTaskStore.getState()
    addToolLog({ taskId: 1, tool: 'grep', args: '{}', createdAt: NOW })
    addToolLog({ taskId: 1, tool: 'read', args: '{}', createdAt: NOW })

    setToolResult(0, { name: 'grep', success: true, output: '2 matches', error: null, durationMs: 12 })

    const logs = useTaskStore.getState().toolLogs
    expect(logs[0].result?.success).toBe(true)
    expect(logs[0].result?.output).toBe('2 matches')
    expect(logs[1].result).toBeUndefined()
  })

  it('loadHistory stores the fetched task list', async () => {
    mockedIpc.listTasks.mockResolvedValue([task])
    await useTaskStore.getState().loadHistory(1)
    expect(mockedIpc.listTasks).toHaveBeenCalledWith(1)
    expect(useTaskStore.getState().history).toEqual([task])
  })

  it('markTaskRolledBack flips one history entry without touching others', () => {
    useTaskStore.setState({
      history: [
        { ...task, id: 1, status: 'done' },
        { ...task, id: 2, status: 'applied' },
      ],
    })

    useTaskStore.getState().markTaskRolledBack(1)

    const history = useTaskStore.getState().history
    expect(history[0].status).toBe('rolledBack')
    expect(history[1].status).toBe('applied')
  })

  it('reset clears everything', () => {
    useTaskStore.getState().setCurrentTask(task)
    useTaskStore
      .getState()
      .handleEvent({ type: 'step', taskId: 1, name: '生成补丁', status: 'running' })
    useTaskStore.getState().handleEvent({ type: 'tool', taskId: 1, tool: 'grep', args: '{}' })

    useTaskStore.getState().reset()

    const state = useTaskStore.getState()
    expect(state.currentTask).toBeNull()
    expect(state.steps).toEqual([])
    expect(state.patches).toEqual([])
    expect(state.verifies).toEqual([])
    expect(state.context).toEqual([])
    expect(state.toolLogs).toEqual([])
    expect(state.events).toEqual([])
  })

  it('initializes nullable token usage and archives tokens without a current task', () => {
    useTaskStore.getState().handleEvent({ type: 'token', taskId: 1, input: 1, output: 2 })
    expect(useTaskStore.getState().currentTask).toBeNull()
    useTaskStore.getState().setCurrentTask({ ...task, tokenInput: null, tokenOutput: null })
    useTaskStore.getState().handleEvent({ type: 'token', taskId: 1, input: 3, output: 4 })
    expect(useTaskStore.getState().currentTask).toMatchObject({ tokenInput: 3, tokenOutput: 4 })
    expect(useTaskStore.getState().events).toHaveLength(2)
  })

  it('keeps history on IPC failure and clears every collection on reset', async () => {
    useTaskStore.getState().setHistory([task])
    mockedIpc.listTasks.mockRejectedValueOnce('offline')
    await expect(useTaskStore.getState().loadHistory(1)).rejects.toBe('offline')
    expect(useTaskStore.getState().history).toEqual([task])
    useTaskStore.getState().setContext([{ path: 'main.ts', content: 'text', reason: 'match', tokens: 1 }])
    useTaskStore.getState().handleEvent({ type: 'patch', taskId: 1, file: 'main.ts', diff: 'diff' })
    useTaskStore.getState().handleEvent({ type: 'verify', taskId: 1, kind: 'test', passed: false, output: 'error' })
    useTaskStore.getState().reset()
    const state = useTaskStore.getState()
    for (const list of [state.history, state.context, state.patches, state.verifies, state.steps, state.events, state.toolLogs]) {
      expect(list).toEqual([])
    }
  })
})
