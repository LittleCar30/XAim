import { useEffect, useRef, useState } from 'react'

import { useConfigStore } from '@/stores/config'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import type { Task, TaskStatus } from '@/types'

/** 这些状态表示任务已结束；其余状态按运行中处理（耗时实时跳动）。 */
const TERMINAL_STATUSES: TaskStatus[] = [
  'idle',
  'applied',
  'done',
  'failed',
  'blocked',
  'rolledBack',
  'cancelled',
]

function formatTokens(n: number): string {
  if (n < 1000) return String(n)
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`
  return `${(n / 1_000_000).toFixed(2)}M`
}

function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const p = (x: number) => String(x).padStart(2, '0')
  return h > 0 ? `${h}:${p(m)}:${p(s)}` : `${p(m)}:${p(s)}`
}

/** 底部状态栏：索引状态、模型名、当前任务耗时（实时）、token 用量与成本。 */
export default function StatusBar({ task }: { task?: Task | null }) {
  const projectLoading = useProjectStore((s) => s.loading)
  const hasProject = useProjectStore((s) => s.currentProject !== null)
  const liveTask = useTaskStore((s) => s.currentTask)
  const currentTask = task === undefined ? liveTask : task
  const modelConfig = useConfigStore((s) => s.config?.model ?? s.modelConfig)
  const loadModelConfig = useConfigStore((s) => s.loadModelConfig)

  // 模型名兜底加载（TopBar 通常已拉取；此处保证独立挂载时也可见）。
  const triedConfigRef = useRef(false)
  useEffect(() => {
    if (triedConfigRef.current) return
    triedConfigRef.current = true
    if (!useConfigStore.getState().config) loadModelConfig().catch(() => {})
  }, [loadModelConfig])

  const running = currentTask !== null && !TERMINAL_STATUSES.includes(currentTask.status)

  // 运行中任务的耗时每秒跳动。
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!running) return
    const timer = setInterval(() => setTick((n) => n + 1), 1000)
    return () => clearInterval(timer)
  }, [running])

  // 耗时优先级：后端回填的 durationMs → finishedAt 差值 → 运行中实时差值。
  const durationMs = (() => {
    if (!currentTask) return null
    if (currentTask.durationMs != null) return currentTask.durationMs
    if (currentTask.finishedAt != null && currentTask.createdAt > 0) {
      return currentTask.finishedAt - currentTask.createdAt
    }
    if (running && currentTask.createdAt > 0) return Date.now() - currentTask.createdAt
    return null
  })()

  const tokenIn = currentTask?.tokenInput ?? 0
  const tokenOut = currentTask?.tokenOutput ?? 0

  // 索引状态：打开/关闭进行中 → 索引中；已打开 → 已索引；否则未索引。
  const indexLabel = projectLoading ? '索引中' : hasProject ? '已索引' : '未索引'
  const indexDot = projectLoading
    ? 'bg-amber-400 animate-pulse'
    : hasProject
      ? 'bg-emerald-400'
      : 'bg-slate-600'

  return (
    <footer
      className="flex h-7 shrink-0 items-center gap-4 border-t border-slate-800 bg-slate-900 px-4 text-[11px] text-slate-500"
    >
      <span className="flex shrink-0 items-center gap-1" title={projectLoading ? '正在建立索引' : hasProject ? '项目索引就绪' : '尚未打开项目'}>
        <span className={`h-1.5 w-1.5 rounded-full ${indexDot}`} aria-hidden="true" />
        {indexLabel}
      </span>

      <span className="min-w-0 truncate" title={modelConfig ? `${modelConfig.provider} / ${modelConfig.name}` : '未配置模型'}>
        {modelConfig ? modelConfig.name : '未配置模型'}
      </span>

      <div className="ml-auto flex shrink-0 items-center gap-4">
        {durationMs != null && (
          <span className="font-mono" title="当前任务耗时">
            {formatDuration(durationMs)}
          </span>
        )}
        {(tokenIn !== 0 || tokenOut !== 0) && (
          <span className="font-mono" title="token 用量（输入/输出）">
            ↑{formatTokens(tokenIn)} ↓{formatTokens(tokenOut)} tok
          </span>
        )}
        {currentTask?.cost != null && (
          <span className="font-mono" title="任务成本">
            ${currentTask.cost.toFixed(4)}
          </span>
        )}
      </div>
    </footer>
  )
}
