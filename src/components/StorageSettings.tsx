import { useCallback, useEffect, useState } from 'react'
import { Archive, FolderOpen, RefreshCw } from 'lucide-react'
import { ipc } from '@/services/ipc'
import { toMessage } from '@/lib/task-format'
import type { StorageInfo } from '@/types'

const buttonCls = 'inline-flex items-center gap-1.5 rounded-md border border-slate-700 px-3 py-1.5 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-50'

function bytes(value: number): string {
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  if (value < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(1)} MB`
  return `${(value / 1024 ** 3).toFixed(2)} GB`
}

export default function StorageSettings() {
  const [info, setInfo] = useState<StorageInfo | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const refresh = useCallback(async () => { setInfo(await ipc.getStorageInfo()) }, [])
  useEffect(() => {
    let active = true
    void (async () => {
      try { const value = await ipc.getStorageInfo(); if (active) setInfo(value) }
      catch (error) { if (active) setError(toMessage(error)) }
    })()
    return () => { active = false }
  }, [])

  const run = async (action: () => Promise<void>) => {
    setBusy(true); setError(null); setMessage(null)
    try { await action(); await refresh() } catch (error) { setError(toMessage(error)) }
    finally { setBusy(false) }
  }

  return <section className="settings-card" aria-label="数据与存储">
    <div className="mb-3 flex items-center justify-between"><h2 className="text-sm font-semibold text-slate-200">数据与存储</h2>
      <button type="button" className={buttonCls} disabled={busy} aria-label="刷新存储信息" onClick={() => void run(refresh)}><RefreshCw size={13} /></button>
    </div>
    {info ? <div className="space-y-3 text-xs text-slate-400">
      <p>配置、历史对话和界面偏好统一备份。任务成果应用到你打开的项目。</p>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2">
        <dt>数据目录</dt><dd className="min-w-0 break-all font-mono text-slate-300">{info.home}</dd>
        <dt>占用空间</dt><dd>{bytes(info.bytes)} · {info.workspaceCount} 个任务工作区</dd>
      </dl>
      {info.notice && <p role="status">{info.notice}</p>}
      <div className="flex flex-wrap gap-2">
        <button type="button" className={buttonCls} disabled={busy} onClick={() => void run(() => ipc.openDataFolder('home'))}><FolderOpen size={13} />打开数据目录</button>
        <button type="button" className={buttonCls} disabled={busy} onClick={() => void run(() => ipc.openDataFolder('workspaces'))}>任务工作区</button>
        <button type="button" className={buttonCls} disabled={busy} onClick={() => void run(() => ipc.openDataFolder('backups'))}>备份目录</button>
        <button type="button" className={buttonCls} disabled={busy} onClick={() => void run(async () => { const path = await ipc.createDataBackup(); setMessage(`备份已保存：${path}`) })}><Archive size={13} />立即备份</button>
        <button type="button" className={buttonCls} disabled={busy || info.environmentOverride} onClick={() => void run(async () => {
          const target = await ipc.restoreDataBackup()
          if (target) setMessage('已安排恢复，重启后使用备份数据；当前数据已自动备份并保留在原目录。')
        })}>恢复备份…</button>
      </div>
      <label className="flex flex-wrap items-center gap-3">已结束任务的工作区保留
        <select className="rounded bg-slate-950 px-2 py-1.5 ring-1 ring-slate-700" value={info.retentionDays} disabled={busy}
          onChange={(event) => { const days = Number(event.target.value); void run(async () => { await ipc.setWorkspaceRetention(days); setMessage('工作区保留期限已保存') }) }}>
          {[7, 30, 90, 0].map((days) => <option key={days} value={days}>{days ? `${days} 天` : '永久保留'}</option>)}
          {![7, 30, 90, 0].includes(info.retentionDays) && <option value={info.retentionDays}>{info.retentionDays} 天</option>}
        </select>
        <button type="button" className={buttonCls} disabled={busy} onClick={() => void run(async () => { const count = await ipc.cleanupWorkspaces(); setMessage(`已清理 ${count} 个过期工作区`) })}>清理过期工作区</button>
      </label>
      <p>启动时自动清理过期工作区；保留对话、补丁和待确认任务。</p>
      <p>备份包含配置、对话和变更记录。API Key 保存在系统凭据库，跨设备恢复时需重新填写。</p>
      {info.pendingHome ? <div className="space-y-2">
        <p className="break-all text-amber-400">下次启动迁移到：{info.pendingHome}。当前设置和任务仍保存到原目录。</p>
        <button type="button" className={buttonCls} disabled={busy} onClick={() => void run(async () => { await ipc.cancelDataMove(); setMessage('已取消迁移') })}>取消迁移</button>
      </div> : <button type="button" className={buttonCls} disabled={busy || info.environmentOverride} onClick={() => void run(async () => {
        const path = await ipc.pickStorageFolder()
        if (path) { await ipc.moveDataDirectory(path); setMessage('已安排迁移，重启 XAim 后生效；原数据目录会保留。') }
      })}>迁移数据目录…</button>}
      {info.environmentOverride && <p>数据目录由 XAIM_HOME 指定。</p>}
    </div> : !error && <p className="text-xs text-slate-500">正在读取存储信息…</p>}
    {message && <p role="status" className="mt-3 break-all text-xs text-emerald-400">{message}</p>}
    {error && <p role="alert" className="mt-3 break-all text-xs text-rose-400">无法完成存储操作：{error}</p>}
  </section>
}
