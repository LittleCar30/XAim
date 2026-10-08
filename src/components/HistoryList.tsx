import { useCallback, useEffect, useState } from 'react'

import {
  formatDuration,
  formatTaskTime,
  formatTokens,
  TASK_STATUS_LABELS,
  taskStatusBadgeCls,
  toMessage,
} from '@/lib/task-format'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import type { Task, TaskStatus } from '@/types'

/** 回滚仅对已应用过补丁的任务有意义（后端对其他状态报"没有已应用的补丁"）。 */
const ROLLBACK_ABLE: TaskStatus[] = ['applied', 'done']

/** 任务是否已到终态（用于触发历史刷新）。 */
function isTerminal(status: TaskStatus): boolean {
  return ['applied', 'done', 'failed', 'rolledBack', 'cancelled'].includes(status)
}

function ChevronIcon({ open }: { open: boolean }) {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={`shrink-0 text-slate-500 transition-transform ${open ? 'rotate-90' : ''}`}
    >
      <polyline points="9 18 15 12 9 6" />
    </svg>
  )
}

function RefreshIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <polyline points="23 4 23 10 17 10" />
      <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
    </svg>
  )
}

/** 历史任务列表：当前项目的任务（最新在前），展开查看详情并可回滚已应用变更。 */
export default function HistoryList() {
  const currentProject = useProjectStore((s) => s.currentProject)
  const currentTaskStatus = useTaskStore((s) => s.currentTask?.status)
  const history = useTaskStore((s) => s.history)
  const loadHistory = useTaskStore((s) => s.loadHistory)
  const setHistory = useTaskStore((s) => s.setHistory)
  const markTaskRolledBack = useTaskStore((s) => s.markTaskRolledBack)

  const [loading, setLoading] = useState(false)
  const [expandedId, setExpandedId] = useState<number | null>(null)
  const [rollingBackId, setRollingBackId] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    if (!currentProject) {
      setHistory([])
      return
    }
    setLoading(true)
    try {
      await loadHistory(currentProject.id)
      setError(null)
    } catch (e) {
      setError(toMessage(e))
    } finally {
      setLoading(false)
    }
  }, [currentProject, loadHistory, setHistory])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    if (currentTaskStatus && isTerminal(currentTaskStatus)) void refresh()
  }, [currentTaskStatus, refresh])

  const rollback = async (task: Task) => {
    setRollingBackId(task.id)
    setError(null)
    try {
      await ipc.rollbackTask(task.id)
      markTaskRolledBack(task.id)
    } catch (e) {
      setError(toMessage(e))
    } finally {
      setRollingBackId(null)
    }
  }

  return (
    <div className="flex h-56 shrink-0 flex-col border-t border-slate-800">
      <div className="flex shrink-0 items-center justify-between px-3 pt-2">
        <p className="text-[11px] text-slate-600">历史任务 · {history.length}</p>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={loading || !currentProject}
          aria-label="刷新历史任务"
          title="刷新"
          className="rounded p-0.5 text-slate-500 hover:bg-slate-800 hover:text-slate-300 disabled:opacity-40"
        >
          <RefreshIcon />
        </button>
      </div>

      {error && (
        <p className="mx-2 mt-1 break-all text-[11px] text-rose-400" title={error}>
          {error}
        </p>
      )}

      {!currentProject ? (
        <p className="px-3 py-2 text-xs text-slate-500">未打开项目</p>
      ) : history.length === 0 ? (
        <p className="px-3 py-2 text-xs text-slate-500">
          {loading ? '加载历史任务…' : '暂无历史任务'}
        </p>
      ) : (
        <ul className="min-h-0 flex-1 overflow-auto p-2">
          {history.map((task) => {
            const open = expandedId === task.id
            const duration =
              task.durationMs != null
                ? task.durationMs
                : task.finishedAt != null
                  ? task.finishedAt - task.createdAt
                  : null
            const canRollback = ROLLBACK_ABLE.includes(task.status)
            return (
              <li key={task.id} className="mb-1 overflow-hidden rounded-md bg-slate-900/60">
                <button
                  type="button"
                  onClick={() => setExpandedId(open ? null : task.id)}
                  aria-expanded={open}
                  title={task.prompt}
                  className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-xs hover:bg-slate-800/60"
                >
                  <ChevronIcon open={open} />
                  <span
                    className={`shrink-0 rounded px-1 text-[10px] leading-4 ${taskStatusBadgeCls(task.status)}`}
                  >
                    {TASK_STATUS_LABELS[task.status]}
                  </span>
                  <span className="min-w-0 truncate text-slate-300">{task.prompt}</span>
                  <span className="ml-auto flex shrink-0 items-center gap-2 font-mono text-[11px] text-slate-600">
                    {duration != null && <span>{formatDuration(duration)}</span>}
                    {task.cost != null && <span>${task.cost.toFixed(4)}</span>}
                  </span>
                </button>
                {open && (
                  <div className="border-t border-slate-800 bg-slate-950 px-2 py-1.5 text-[11px] leading-4">
                    <p className="whitespace-pre-wrap break-all text-slate-300">{task.prompt}</p>
                    <dl className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5 text-slate-500">
                      <dt>状态</dt>
                      <dd className="text-slate-400">{TASK_STATUS_LABELS[task.status]}</dd>
                      <dt>创建</dt>
                      <dd className="font-mono text-slate-400">{formatTaskTime(task.createdAt)}</dd>
                      {task.finishedAt != null && (
                        <>
                          <dt>完成</dt>
                          <dd className="font-mono text-slate-400">
                            {formatTaskTime(task.finishedAt)}
                          </dd>
                        </>
                      )}
                      {duration != null && (
                        <>
                          <dt>耗时</dt>
                          <dd className="font-mono text-slate-400">{formatDuration(duration)}</dd>
                        </>
                      )}
                      {(task.tokenInput !== 0 || task.tokenOutput !== 0) && (
                        <>
                          <dt>Token</dt>
                          <dd className="font-mono text-slate-400">
                            ↑{formatTokens(task.tokenInput ?? 0)} ↓
                            {formatTokens(task.tokenOutput ?? 0)}
                          </dd>
                        </>
                      )}
                      {task.cost != null && (
                        <>
                          <dt>成本</dt>
                          <dd className="font-mono text-slate-400">${task.cost.toFixed(4)}</dd>
                        </>
                      )}
                      {task.intent && (
                        <>
                          <dt>意图</dt>
                          <dd className="break-all text-slate-400">{task.intent}</dd>
                        </>
                      )}
                    </dl>
                    {canRollback && (
                      <button
                        type="button"
                        onClick={() => void rollback(task)}
                        disabled={rollingBackId === task.id}
                        title="回滚该任务已应用的全部变更"
                        className="mt-1.5 w-full rounded-md border border-rose-500/40 px-2 py-1 text-rose-400 hover:bg-rose-500/10 disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        {rollingBackId === task.id ? '回滚中…' : '回滚此任务的变更'}
                      </button>
                    )}
                  </div>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
