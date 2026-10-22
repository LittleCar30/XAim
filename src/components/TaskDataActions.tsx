import { useState } from 'react'
import { Download, Folder, FolderOpen } from 'lucide-react'
import { ipc } from '@/services/ipc'
import { toMessage } from '@/lib/task-format'
import { useConversationStore } from '@/stores/conversation'
import { useTaskStore } from '@/stores/task'
import { useProjectStore } from '@/stores/project'
import type { Patch, Task } from '@/types'

export default function TaskDataActions({ task, patches, historical }: { task: Task; patches: Patch[]; historical: boolean }) {
  const taskId = task.id
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState(false)
  const run = async (action: () => Promise<void>) => {
    setBusy(true); setNotice(null); setError(false)
    try { await action() } catch (error) { setError(true); setNotice(toMessage(error)) }
    finally { setBusy(false) }
  }
  const review = async (action: () => Promise<void>) => {
    await action()
    await useConversationStore.getState().select(taskId)
    await useTaskStore.getState().loadHistory(task.projectId)
    await useProjectStore.getState().loadFileTree(task.projectId)
  }
  return <div className="mt-4 text-xs text-slate-500">
    <div className="flex flex-wrap gap-3">
      {historical && task.status === 'waitingConfirm' && patches[0] && <>
        <button type="button" disabled={busy} className="text-emerald-400 disabled:opacity-50" onClick={() => void run(() => review(() => ipc.applyPatch(patches[0].id)))}>应用变更</button>
        <button type="button" disabled={busy} className="hover:text-slate-200 disabled:opacity-50" onClick={() => void run(() => review(() => ipc.rejectPatch(patches[0].id)))}>拒绝变更</button>
      </>}
      {historical && ['done', 'applied'].includes(task.status) && patches.some((patch) => patch.applied) &&
        <button type="button" disabled={busy} className="text-rose-400 disabled:opacity-50" onClick={() => void run(() => review(() => ipc.rollbackTask(taskId)))}>撤销此任务变更</button>}
      <button type="button" disabled={busy} className="inline-flex items-center gap-1 hover:text-slate-200 disabled:opacity-50" onClick={() => void run(async () => {
        const path = await ipc.exportTaskRecord(taskId)
        if (path) setNotice(`任务记录已导出：${path}`)
      })}><Download size={13} />导出对话</button>
      <button type="button" disabled={busy} className="inline-flex items-center gap-1 hover:text-slate-200 disabled:opacity-50" onClick={() => void run(() => ipc.openTaskWorkspace(taskId))}><FolderOpen size={13} />打开任务副本</button>
      <button type="button" disabled={busy} className="inline-flex items-center gap-1 hover:text-slate-200 disabled:opacity-50" onClick={() => void run(() => ipc.openTaskProject(taskId))}><Folder size={13} />打开原项目</button>
    </div>
    {notice && <p role={error ? 'alert' : 'status'} className={`mt-2 break-all ${error ? 'text-rose-400' : ''}`}>{notice}</p>}
  </div>
}
