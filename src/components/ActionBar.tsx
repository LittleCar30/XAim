import { useState } from 'react'

import { ipc } from '@/services/ipc'
import { useTaskStore } from '@/stores/task'

/** IPC 抛出值的统一展示消息（Tauri 命令失败时 reject 的是 string）。 */
function toMessage(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  return String(e)
}

/** 操作栏：对候选补丁的应用/拒绝（逐个走 IPC）与任务级回滚。 */
export default function ActionBar() {
  const currentTask = useTaskStore((s) => s.currentTask)
  const patches = useTaskStore((s) => s.patches)
  const setCurrentTask = useTaskStore((s) => s.setCurrentTask)
  const setPatchApplied = useTaskStore((s) => s.setPatchApplied)
  const removePatch = useTaskStore((s) => s.removePatch)

  const [busy, setBusy] = useState<'apply' | 'reject' | 'rollback' | null>(null)
  const [error, setError] = useState<string | null>(null)

  const unapplied = patches.filter((p) => !p.applied)
  const applied = patches.filter((p) => p.applied)
  const disabled = busy !== null

  const run = async (kind: 'apply' | 'reject' | 'rollback', fn: () => Promise<void>) => {
    setBusy(kind)
    setError(null)
    try {
      await fn()
    } catch (e) {
      setError(`${{ apply: '应用', reject: '拒绝', rollback: '回滚' }[kind]}失败：${toMessage(e)}`)
    } finally {
      setBusy(null)
    }
  }

  const handleApply = () =>
    run('apply', async () => {
      const tasks = [...new Map(unapplied.map((patch) => [patch.taskId, patch])).values()]
      for (const patch of tasks) {
        await ipc.applyPatch(patch.id)
        for (const file of unapplied.filter((file) => file.taskId === patch.taskId)) setPatchApplied(file.id, true)
      }
    })

  const handleReject = () =>
    run('reject', async () => {
      const tasks = [...new Map(unapplied.map((patch) => [patch.taskId, patch])).values()]
      for (const patch of tasks) {
        await ipc.rejectPatch(patch.id)
        for (const file of unapplied.filter((file) => file.taskId === patch.taskId)) removePatch(file.id)
      }
    })

  const handleRollback = () =>
    run('rollback', async () => {
      if (!currentTask) return
      await ipc.rollbackTask(currentTask.id)
      for (const patch of applied) setPatchApplied(patch.id, false)
      setCurrentTask({ ...currentTask, status: 'rolledBack' })
    })

  return (
    <div className="task-action-bar flex shrink-0 items-center gap-2 border-t border-slate-800 px-3 py-2">
      <button
        type="button"
        onClick={handleApply}
        disabled={disabled || unapplied.length === 0}
        title={unapplied.length === 0 ? '没有待应用的候选补丁' : `应用 ${unapplied.length} 个候选补丁`}
        className="rounded-md bg-emerald-500/90 px-3 py-1 text-xs font-medium text-slate-950 hover:bg-emerald-400 disabled:cursor-not-allowed disabled:opacity-40"
      >
        {busy === 'apply' ? '应用中…' : `应用${unapplied.length > 0 ? `（${unapplied.length}）` : ''}`}
      </button>
      <button
        type="button"
        onClick={handleReject}
        disabled={disabled || unapplied.length === 0}
        title={unapplied.length === 0 ? '没有待拒绝的候选补丁' : `拒绝 ${unapplied.length} 个候选补丁`}
        className="rounded-md bg-slate-800 px-3 py-1 text-xs text-slate-200 hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-40"
      >
        {busy === 'reject' ? '拒绝中…' : `拒绝${unapplied.length > 0 ? `（${unapplied.length}）` : ''}`}
      </button>
      <button
        type="button"
        onClick={handleRollback}
        disabled={disabled || applied.length === 0}
        title={applied.length === 0 ? '没有已应用的变更可回滚' : '回滚本任务已应用的全部变更'}
        className="rounded-md bg-slate-800 px-3 py-1 text-xs text-slate-200 hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-40"
      >
        {busy === 'rollback' ? '回滚中…' : '回滚'}
      </button>
      {error && (
        <p className="ml-auto min-w-0 truncate text-[11px] text-rose-400" title={error}>
          {error}
        </p>
      )}
    </div>
  )
}
