import { ipc } from '@/services/ipc'
import { useConfigStore } from '@/stores/config'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import type { Config, FileNode, Patch, Project, Task, TaskStatus, TaskStep } from '@/types'
import { useConversationStore } from '@/stores/conversation'

const original = `export function average(values: number[]): number {
  const total = values.reduce((sum, value) => sum + value, 0)
  return total / values.length
}
`
const modified = `export function average(values: number[]): number {
  if (values.length === 0) return 0
  const total = values.reduce((sum, value) => sum + value, 0)
  return total / values.length
}
`
const files: Record<string, string> = {
  'src/utils/average.ts': original,
  'src/utils/average.test.ts': `import { expect, it } from 'vitest'
import { average } from './average'

it('returns zero for an empty array', () => {
  expect(average([])).toBe(0)
})

it('calculates the average', () => {
  expect(average([2, 4, 6])).toBe(4)
})
`,
  'src/App.tsx': `import { average } from './utils/average'

export default function App() {
  return <div>Average: {average([2, 4, 6])}</div>
}
`,
  'src/main.tsx': `import { createRoot } from 'react-dom/client'
import App from './App'

createRoot(document.getElementById('root')!).render(<App />)
`,
  'package.json': JSON.stringify({ name: 'sample-app', scripts: { test: 'vitest run', build: 'tsc && vite build' } }, null, 2),
  'tsconfig.json': JSON.stringify({ compilerOptions: { strict: true, jsx: 'react-jsx' } }, null, 2),
  'README.md': '# Sample App\n\nA small TypeScript project.\n',
}

function directory(name: string, path: string, children: FileNode[]): FileNode {
  return { name, path, children, isDir: true, size: null }
}

function file(path: string): FileNode {
  return { name: path.split('/').pop()!, path, children: [], isDir: false, size: files[path].length }
}

export function setupPreview(): void {
  const now = Date.now()
  let project: Project = { id: 1, name: 'sample-app', path: 'preview/sample-app', createdAt: now, lastOpenedAt: now }
  const tree = directory(project.name, '.', [
    directory('src', 'src', [directory('utils', 'src/utils', [file('src/utils/average.ts'), file('src/utils/average.test.ts')]), file('src/App.tsx'), file('src/main.tsx')]),
    file('package.json'), file('tsconfig.json'), file('README.md'),
  ])
  let config: Config = {
    model: { provider: 'deepseek', name: 'deepseek-chat', apiKey: null, baseUrl: null, temperature: 0.7, maxTokens: 4096 },
    router: { fastModel: 'deepseek-chat', strongModel: 'deepseek-chat' },
  }
  config.modelProfiles = [
    { id: 'preview-deepseek', label: 'DeepSeek Chat', config: config.model, supportsReasoning: false, reasoningEffort: null },
    { id: 'preview-local', label: '本地编码模型', config: { provider: 'ollama', name: 'qwen2.5-coder', apiKey: null, baseUrl: null, temperature: 0.2, maxTokens: 4096 }, supportsReasoning: false, reasoningEffort: null },
    { id: 'preview-reasoning', label: '推理模型', config: { provider: 'openai', name: 'o3-mini', apiKey: 'preview-only', baseUrl: null, temperature: 0.7, maxTokens: 8192 }, supportsReasoning: true, reasoningEffort: 'medium' },
  ]
  config.activeModelId = 'preview-deepseek'
  let sequence = 100
  let generation = 0
  const tasks = new Map<number, Task>()
  const patches = new Map<number, Patch[]>()
  const taskSteps = new Map<number, TaskStep[]>()
  tasks.set(99, { id: 99, projectId: 1, prompt: '检查 average 函数与现有测试', status: 'done', intent: null, plan: null,
    createdAt: now - 3600000, finishedAt: now - 3590000, durationMs: 10000, tokenInput: 1200, tokenOutput: 180, cost: 0.0005 })
  patches.set(99, [])
  taskSteps.set(99, [
    { id: 1, taskId: 99, stepType: '读取 average 实现', content: 'done', createdAt: now - 3599000 },
    { id: 2, taskId: 99, stepType: '检查测试覆盖范围', content: 'done', createdAt: now - 3595000 },
  ])
  const stages: { name: string; status: TaskStatus }[] = [
    { name: '理解任务：定位空数组返回 NaN 的原因', status: 'parsing' },
    { name: '检索上下文：读取实现与测试文件', status: 'retrieving' },
    { name: '制定方案：添加空数组边界检查', status: 'planning' },
    { name: '生成补丁：保持现有平均值计算逻辑', status: 'generating' },
    { name: '验证候选：类型检查与 2 项测试通过', status: 'verifying' },
  ]

  function context() {
    useTaskStore.getState().setContext([
      { path: 'src/utils/average.ts', content: files['src/utils/average.ts'], reason: '目标函数 average 的实现', tokens: 84 },
      { path: 'src/utils/average.test.ts', content: files['src/utils/average.test.ts'], reason: '覆盖空数组与正常输入的测试', tokens: 136 },
    ])
  }

  function finish(status: TaskStatus) {
    const task = useTaskStore.getState().currentTask
    if (!task) return
    const next = { ...task, status, finishedAt: Date.now(), durationMs: Date.now() - task.createdAt }
    tasks.set(next.id, next)
    useTaskStore.getState().setCurrentTask(next)
    useTaskStore.getState().setHistory([...tasks.values()].reverse())
  }

  function addResults(taskId: number) {
    const patch: Patch = {
      id: taskId, taskId, filePath: 'src/utils/average.ts', applied: false, score: 0.96, createdAt: Date.now(),
      diff: '--- a/src/utils/average.ts\n+++ b/src/utils/average.ts\n@@ -1,4 +1,5 @@\n export function average(values: number[]): number {\n+  if (values.length === 0) return 0\n   const total = values.reduce((sum, value) => sum + value, 0)\n   return total / values.length\n }',
    }
    patches.set(taskId, [patch])
    useTaskStore.setState({ patches: [patch], verifies: [
      { kind: 'build', passed: true, output: 'tsc --noEmit\nType check passed.', durationMs: 1240 },
      { kind: 'test', passed: true, output: 'vitest run\n average.test.ts (2 tests)\n Tests 2 passed (2)', durationMs: 680 },
    ] })
    useTaskStore.getState().addToolLog({ taskId, tool: 'read_file', args: JSON.stringify({ path: 'src/utils/average.ts' }), createdAt: now,
      result: { name: 'read_file', success: true, output: original, error: null, durationMs: 12 } })
    useTaskStore.getState().addToolLog({ taskId, tool: 'exec', args: JSON.stringify({ command: 'pnpm test' }), createdAt: Date.now(),
      result: { name: 'exec', success: true, output: '2 tests passed', error: null, durationMs: 680 } })
    finish('waitingConfirm')
  }

  function start(prompt: string, ready: boolean): number {
    useConversationStore.getState().archiveCurrent()
    const currentGeneration = ++generation
    const id = ++sequence
    const task: Task = { id, projectId: 1, prompt, status: 'parsing', intent: '修复空数组边界情况', plan: null,
      createdAt: Date.now(), finishedAt: null, durationMs: null, tokenInput: 2840, tokenOutput: 386, cost: 0.0012 }
    tasks.set(id, task)
    patches.set(id, [])
    taskSteps.set(id, [])
    useTaskStore.getState().reset()
    useTaskStore.getState().setCurrentTask(task)
    useTaskStore.getState().setHistory([...tasks.values()].reverse())
    context()
    const advance = (index: number) => {
      if (generation !== currentGeneration) return
      const step = stages[index]
      const next = { ...task, status: step.status }
      tasks.set(id, next)
      useTaskStore.getState().setCurrentTask(next)
      const record = { id: index, taskId: id, stepType: step.name, content: 'done', createdAt: Date.now() }
      taskSteps.get(id)!.push(record)
      useTaskStore.getState().addStep(record)
      if (index === stages.length - 1) addResults(id)
    }
    stages.forEach((_, index) => {
      if (ready) advance(index)
      else window.setTimeout(() => advance(index), 600 + index * 900)
    })
    return id
  }

  const previewIpc: typeof ipc = {
    getStorageInfo: async () => { throw new Error('请在桌面应用中管理本地数据') },
    openDataFolder: async () => { throw new Error('浏览器预览无法打开本地数据目录') },
    pickStorageFolder: async () => { throw new Error('浏览器预览无法选择本地数据目录') },
    moveDataDirectory: async () => { throw new Error('请在桌面应用中迁移数据') },
    cancelDataMove: async () => { throw new Error('请在桌面应用中管理数据迁移') },
    createDataBackup: async () => { throw new Error('请在桌面应用中备份数据') },
    restoreDataBackup: async () => { throw new Error('请在桌面应用中恢复备份') },
    setWorkspaceRetention: async () => { throw new Error('请在桌面应用中设置保留期限') },
    cleanupWorkspaces: async () => { throw new Error('请在桌面应用中清理工作区') },
    openTaskWorkspace: async () => { throw new Error('示例任务没有本地工作区') },
    exportTaskRecord: async () => { throw new Error('请在桌面应用中导出对话') },
    getPreferences: async () => ({}),
    setPreference: async () => {},
    getRuntimeSettings: async () => ({ defaults: {}, shared: {}, local: {}, effective: { taskTimeoutSec: 1800, maxTurns: 32 } }),
    setRuntimeSettings: async () => { throw new Error('请在桌面应用中保存任务设置') },
    pickProjectFolder: async () => { throw new Error('浏览器预览无法打开系统文件夹选择器，请在桌面应用中使用') },
    openProject: async () => project,
    closeProject: async () => { generation++; useTaskStore.getState().reset() },
    listProjects: async () => [project],
    getFileTree: async () => tree,
    readFile: async (_, path, start = 1, end) => {
      if (!(path in files)) throw new Error('示例中没有此文件')
      return files[path].split('\n').slice(start - 1, end).join('\n')
    },
    runTask: async (_, prompt) => start(prompt, false),
    cancelTask: async () => { generation++; finish('cancelled') },
    getTask: async (id) => {
      const task = tasks.get(id)
      if (!task) throw new Error('示例任务不存在')
      return task
    },
    listTasks: async () => [...tasks.values()].reverse(),
    listTaskSteps: async (id) => taskSteps.get(id) ?? [],
    listPatches: async (id) => patches.get(id) ?? [],
    applyPatch: async (id) => {
      const candidates = patches.get(id)
      if (!candidates?.length) throw new Error('示例补丁不存在')
      candidates.forEach((patch) => { patch.applied = true })
      files['src/utils/average.ts'] = modified
      context()
      finish('applied')
    },
    rejectPatch: async (id) => { patches.set(id, []); finish('cancelled') },
    rollbackTask: async (id) => {
      files['src/utils/average.ts'] = original
      patches.get(id)?.forEach((patch) => { patch.applied = false })
      const task = tasks.get(id)
      if (task) tasks.set(id, { ...task, status: 'rolledBack' })
      if (useTaskStore.getState().currentTask?.id === id) finish('rolledBack')
      context()
    },
    getConfig: async () => config,
    setConfig: async (next) => { config = next },
    getModelConfig: async () => config.model,
    setModelConfig: async (model) => { config = { ...config, model } },
    testModelConnection: async () => true,
    listModels: async () => ['deepseek-chat', 'deepseek-reasoner'],
    listProviders: async () => ['deepseek', 'openai', 'ollama', 'newapi'],
  }
  Object.assign(ipc, previewIpc)
  // Preview state and IPC replacements only exist in this browser session.
  project = { ...project, name: 'sample-app · 预览' }
  useProjectStore.setState({ currentProject: project, fileTree: tree, recentProjects: [project], loading: false })
  useConfigStore.setState({ config, modelConfig: config.model, loading: false })
  start('修复 average 在空数组输入时返回 NaN 的问题，并补充边界测试', true)
}
