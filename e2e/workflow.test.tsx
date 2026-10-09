import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { createInterface } from 'node:readline'
import { readFile, access } from 'node:fs/promises'
import { join } from 'node:path'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterAll, beforeAll, expect, it, vi } from 'vitest'

import App from '@/App'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { resetStores } from '@/test/fixtures'
import type { XaimEvent } from '@/types'

const transport = vi.hoisted(() => ({
  invoke: vi.fn(),
  handlers: new Set<(event: { event: string; id: number; payload: XaimEvent }) => void>(),
}))

// Replace only the native transport; IPC wrappers, stores and application stay real.
vi.mock('@tauri-apps/api/core', () => ({ invoke: transport.invoke }))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (channel: string, handler: (event: { event: string; id: number; payload: XaimEvent }) => void) => {
    expect(channel).toBe('xaim://event')
    transport.handlers.add(handler)
    return () => transport.handlers.delete(handler)
  }),
}))
vi.mock('@/lib/monaco', () => ({}))
vi.mock('@monaco-editor/react', () => ({
  DiffEditor: ({ original, modified }: { original: string; modified: string }) => (
    <div aria-label="Diff editor"><pre data-testid="original">{original}</pre><pre data-testid="modified">{modified}</pre></div>
  ),
}))

interface Inspection {
  content: string
  files: { path: string; language: string; hash: string }[]
  gitStatus: string[]
  modelCalls: number
  modelRequests: { model: string; stream: boolean; reasoning_effort?: string; max_tokens?: number; max_completion_tokens?: number; temperature?: number }[]
}
interface Verification { command: string; passed: boolean; output: string }
interface Response { id: number; result: unknown; error?: string; events: XaimEvent[] }

let child: ChildProcessWithoutNullStreams
let projectPath: string
let nextId = 0
let output = ''
const pending = new Map<number, { resolve: (result: unknown) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>()
let closed: Promise<number | null>

function request<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  const id = ++nextId
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id)
      reject(new Error(`Rust command timed out: ${command}\n${output}`))
    }, 90_000)
    pending.set(id, { resolve: (result) => resolve(result as T), reject, timer })
    child.stdin.write(`${JSON.stringify({ id, command, args })}\n`)
  })
}

beforeAll(async () => {
  const executable = process.env.XAIM_E2E_EXECUTABLE
  if (!executable) throw new Error('Run this suite with pnpm test:e2e')
  child = spawn(executable, ['commands::e2e::frontend_bridge', '--exact', '--ignored', '--nocapture'], {
    env: { ...process.env, XAIM_E2E_BRIDGE: '1' }, stdio: 'pipe',
  })
  closed = new Promise((resolve) => child.once('close', resolve))
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`Rust bridge did not start\n${output}`)), 30_000)
    child.once('error', reject)
    child.once('close', (code) => {
      clearTimeout(timer)
      const error = new Error(`Rust bridge exited: ${code}\n${output}`)
      for (const item of pending.values()) { clearTimeout(item.timer); item.reject(error) }
      pending.clear()
      reject(error)
    })
    child.stderr.on('data', (data) => { output += data.toString() })
    createInterface({ input: child.stdout }).on('line', (line) => {
      if (!line.startsWith('XAIM_E2E ')) { output += `${line}\n`; return }
      const message = JSON.parse(line.slice('XAIM_E2E '.length))
      if (message.ready) { projectPath = message.projectPath; clearTimeout(timer); resolve(); return }
      const response = message as Response
      const item = pending.get(response.id)
      if (!item) return
      clearTimeout(item.timer)
      pending.delete(response.id)
      // These are serialized events emitted by the real Rust orchestrator.
      act(() => {
        for (const event of response.events) {
          for (const handler of transport.handlers) handler({ event: 'xaim://event', id: response.id, payload: event })
        }
      })
      if (response.error) item.reject(new Error(response.error))
      else item.resolve(response.result)
    })
  })
  transport.invoke.mockImplementation(request)
})

afterAll(async () => {
  if (!child) return
  if (child.exitCode === null && child.signalCode === null) {
    child.stdin.end(`${JSON.stringify({ command: 'shutdown' })}\n`)
    const timer = setTimeout(() => child.kill(), 10_000)
    const code = await closed
    clearTimeout(timer)
    expect(code, output).toBe(0)
  }
  for (const item of pending.values()) clearTimeout(item.timer)
  if (projectPath) await expect(access(projectPath)).rejects.toThrow()
})

it('opens a temporary project, indexes it, submits a task, displays diff, applies it and verifies the real result', async () => {
  resetStores()
  localStorage.clear()
  window.location.hash = '#/'
  const baseline = await request<Verification>('e2e_verify')
  expect(baseline.command).toBe('cargo test')
  expect(baseline.passed).toBe(false)
  expect(baseline.output).toContain('adds_numbers')

  const app = render(<App />)
  await waitFor(() => expect(transport.handlers.size).toBe(1))
  expect(screen.getByRole('textbox', { name: '任务输入' })).toBeDisabled()
  fireEvent.click(screen.getByRole('button', { name: '打开项目' }))
  fireEvent.change(screen.getByRole('textbox', { name: '项目路径' }), { target: { value: projectPath } })
  fireEvent.click(screen.getByRole('button', { name: '打开' }))
  await waitFor(() => expect(screen.getByTitle('项目索引就绪（增量监听中）')).toHaveTextContent('已索引'))
  const projectId = useProjectStore.getState().currentProject!.id
  const indexed = await request<Inspection>('e2e_inspect', { projectId })
  expect(indexed.files).toContainEqual(expect.objectContaining({ path: 'src/lib.rs', language: 'rust', hash: expect.any(String) }))

  const config = await ipc.getConfig()
  await act(async () => {
    const selected = { ...config.model, name: 'selected-reasoner' }
    await ipc.setConfig({ ...config, model: selected, activeModelId: 'selected', modelProfiles: [
      { id: 'old', label: 'Original', config: config.model, supportsReasoning: false, reasoningEffort: null },
      { id: 'selected', label: 'Selected reasoner', config: selected, supportsReasoning: true, reasoningEffort: 'high' },
    ] })
    await (await import('@/stores/config')).useConfigStore.getState().loadConfig()
  })
  expect(screen.getByRole('button', { name: '选择模型' })).toHaveTextContent('Selected reasoner')
  expect(screen.getByRole('button', { name: '选择思考强度' })).toHaveTextContent('高')

  const prompt = 'Fix add so that it returns the sum of a and b'
  fireEvent.change(screen.getByRole('textbox', { name: '任务输入' }), { target: { value: prompt } })
  fireEvent.click(screen.getByRole('checkbox', { name: '运行项目检查' }))
  fireEvent.click(screen.getByRole('button', { name: '发送' }))
  await waitFor(() => expect(useTaskStore.getState().currentTask?.status).toBe('waitingConfirm'), { timeout: 60_000 })
  expect(screen.getByText('等待确认', { selector: 'header > span' })).toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: /src\/lib.rs.*候选/ }))
  expect(await screen.findByTestId('original')).toHaveTextContent('a - b')
  expect(screen.getByTestId('modified')).toHaveTextContent('a + b')
  expect(await readFile(join(projectPath, 'src/lib.rs'), 'utf8')).toContain('a - b')
  const patch = useTaskStore.getState().patches[0]
  expect(patch.id).toBeGreaterThan(0)
  expect(patch.applied).toBe(false)
  expect(useTaskStore.getState().steps.some((step) => step.stepType === '执行工具循环' && step.content === 'done')).toBe(true)
  expect(useTaskStore.getState().toolLogs).toHaveLength(3)
  expect(useTaskStore.getState().toolLogs.every((log) => log.result?.success)).toBe(true)

  fireEvent.click(screen.getByRole('button', { name: '应用（1）' }))
  await waitFor(() => expect(useTaskStore.getState().patches[0].applied).toBe(true), { timeout: 30_000 })
  expect(transport.invoke).toHaveBeenCalledWith('apply_patch', { patchId: patch.id })
  expect(useTaskStore.getState().currentTask?.status).toBe('done')
  expect(screen.getByText('已完成', { selector: 'header > span' })).toBeInTheDocument()
  expect(await readFile(join(projectPath, 'src/lib.rs'), 'utf8')).toContain('a + b')
  const verified = await request<Verification>('e2e_verify')
  expect(verified.passed, verified.output).toBe(true)
  expect(verified.output).toContain('1 passed')
  const final = await request<Inspection>('e2e_inspect', { projectId })
  expect(final.gitStatus).toContain('M src/lib.rs')
  expect(final.modelCalls).toBe(4)
  expect(final.modelRequests).toHaveLength(4)
  for (const call of final.modelRequests) {
    expect(call.stream).toBe(true)
    expect(call.model).toBe('selected-reasoner')
    expect(call.reasoning_effort).toBe('high')
    expect(call.max_completion_tokens).toBeGreaterThan(0)
    expect(call.max_tokens).toBeUndefined()
    expect(call.temperature).toBeUndefined()
  }
  await request('e2e_assert_model')
  const persisted = await ipc.listPatches(useTaskStore.getState().currentTask!.id)
  expect(persisted[0]).toMatchObject({ id: patch.id, applied: true })
  expect(useTaskStore.getState().events.some((event) => event.type === 'error')).toBe(false)
  expect(useTaskStore.getState().events.some((event) => event.type === 'modelText' && event.status === 'complete')).toBe(true)
  const taskId = useTaskStore.getState().currentTask!.id
  fireEvent.click(screen.getByRole('button', { name: '新对话' }))
  fireEvent.click(await screen.findByRole('button', { name: new RegExp(prompt) }))
  expect(await screen.findByRole('button', { name: '返回当前对话' })).toBeInTheDocument()
  await waitFor(() => expect(screen.getByText(prompt, { selector: 'p' })).toBeInTheDocument())
  expect(transport.invoke).toHaveBeenCalledWith('list_task_steps', { taskId })
  expect(screen.queryByRole('button', { name: '回滚' })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: '切换侧栏' }))
  fireEvent.click(screen.getByRole('button', { name: '切换侧栏' }))
  expect(screen.getByRole('button', { name: '返回当前对话' })).toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: '返回当前对话' }))
  expect(screen.getByRole('textbox', { name: '任务输入' })).toBeEnabled()
  app.unmount()
  expect(transport.handlers.size).toBe(0)
})
