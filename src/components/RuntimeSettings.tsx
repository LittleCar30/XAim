import { useEffect, useState } from 'react'
import { ipc } from '@/services/ipc'
import { toMessage } from '@/lib/task-format'
import { useProjectStore } from '@/stores/project'
import type { RuntimeSettings as Settings } from '@/types'

export default function RuntimeSettings() {
  const project = useProjectStore((s) => s.currentProject)
  const [scope, setScope] = useState<'user' | 'project'>('user')
  const [settings, setSettings] = useState<Settings | null>(null)
  const [timeout, setTimeoutValue] = useState('1800')
  const [turns, setTurns] = useState('32')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  const projectId = scope === 'project' ? project?.id ?? null : null
  const fill = (value: Settings) => { setSettings(value); setTimeoutValue(String(value.effective.taskTimeoutSec)); setTurns(String(value.effective.maxTurns)) }
  useEffect(() => {
    let active = true
    setBusy(true); setError(null); setSaved(false); setSettings(null)
    void (async () => {
      try { const value = await ipc.getRuntimeSettings(projectId); if (active) fill(value) }
      catch (error) { if (active) setError(toMessage(error)) }
      finally { if (active) setBusy(false) }
    })()
    return () => { active = false }
  }, [projectId])
  const save = async (inherit = false) => {
    const seconds = Number(timeout), rounds = Number(turns)
    if (!inherit && (!Number.isInteger(seconds) || seconds < 30 || seconds > 86400 || !Number.isInteger(rounds) || rounds < 1 || rounds > 256)) {
      setError('任务时限应为 30–86400 秒，工具循环轮数应为 1–256'); return
    }
    setBusy(true); setError(null); setSaved(false)
    try {
      await ipc.setRuntimeSettings(projectId, inherit ? {} : { taskTimeoutSec: seconds, maxTurns: rounds })
      fill(await ipc.getRuntimeSettings(projectId)); setSaved(true)
    } catch (error) { setError(toMessage(error)) } finally { setBusy(false) }
  }
  return <section className="rounded-lg border border-slate-800 bg-slate-900/50 p-4" aria-label="任务执行设置">
    <h2 className="mb-3 text-sm font-semibold text-slate-200">任务执行</h2>
    <label className="mb-3 flex items-center gap-3 text-xs text-slate-400">适用范围<select aria-label="任务设置范围" value={scope} disabled={busy} className="rounded bg-slate-950 px-2 py-1.5 ring-1 ring-slate-700" onChange={(e) => setScope(e.target.value as 'user' | 'project')}>
      <option value="user">所有项目默认</option><option value="project" disabled={!project}>当前项目{project ? ` · ${project.name}` : ''}</option>
    </select></label>
    <fieldset disabled={busy || !settings} className="space-y-3 text-xs text-slate-400">
      <div className="grid grid-cols-2 gap-3">
        <label>任务总时限（秒）<input type="number" min={30} max={86400} value={timeout} onChange={(e) => { setTimeoutValue(e.target.value); setSaved(false) }} className="mt-1 w-full rounded bg-slate-950 p-2 ring-1 ring-slate-700" /></label>
        <label>工具循环最大轮数<input type="number" min={1} max={256} value={turns} onChange={(e) => { setTurns(e.target.value); setSaved(false) }} className="mt-1 w-full rounded bg-slate-950 p-2 ring-1 ring-slate-700" /></label>
      </div>
      <p>{scope === 'project' ? '个人项目设置优先于项目共享配置和全局默认，保存在本机，不进入 Git。' : '保存后用于下一次任务；项目可以单独覆盖。'}</p>
      <div className="flex items-center gap-3"><button type="button" onClick={() => void save()} className="rounded border border-slate-700 px-3 py-1.5 hover:bg-slate-800">保存任务设置</button>
        {scope === 'project' && <button type="button" onClick={() => void save(true)}>恢复继承设置</button>}
        {saved && <span role="status" className="text-emerald-400">任务设置已保存</span>}
      </div>
    </fieldset>
    {error && <p role="alert" className="mt-3 text-xs text-rose-400">{error}</p>}
  </section>
}
