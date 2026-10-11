import { useEffect, useState } from 'react'
import { ShieldCheck } from 'lucide-react'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import type { ExecutionMode, ExecutionSettings, GitNetwork, PermissionDecision } from '@/types'

const labels: Record<ExecutionMode, string> = { disabled: '不执行程序', ask: '按需询问', project: '按项目授权' }
const defaults: ExecutionSettings = { mode: 'ask', grants: [], gitNetwork: { mode: 'system', url: '' } }
const message = (error: unknown) => error instanceof Error ? error.message : String(error)

export default function ExecutionPermissions({ running }: { running: boolean }) {
  const projectId = useProjectStore((s) => s.currentProject?.id)
  const pending = useTaskStore((s) => s.pendingPermission)
  const [settings, setSettings] = useState<ExecutionSettings>(defaults)
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [respondingTo, setRespondingTo] = useState<string | null>(null)
  const deciding = respondingTo !== null && respondingTo === pending?.id
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [proxyUrl, setProxyUrl] = useState('')
  const [proxyMode, setProxyMode] = useState<GitNetwork['mode']>('system')

  useEffect(() => {
    let active = true
    setSettings(defaults)
    setOpen(false)
    setError('')
    setProxyUrl('')
    setProxyMode('system')
    if (projectId === undefined) return
    setLoading(true)
    void ipc.getExecutionSettings(projectId).then((value) => {
      if (active) { setSettings(value); setProxyUrl(value.gitNetwork.url); setProxyMode(value.gitNetwork.mode) }
    }).catch((e) => { if (active) setError(message(e)) }).finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [projectId, pending?.id])

  useEffect(() => {
    let active = true
    void ipc.getPendingPermission().then((request) => {
      const state = useTaskStore.getState()
      if (active && request && state.currentTask?.id === request.taskId && !state.pendingPermission &&
          !state.events.some((event) => event.taskId === request.taskId && (event.type === 'done' || event.type === 'error' || event.type === 'permissionResolved' && event.requestId === request.id))) {
        useTaskStore.setState({ pendingPermission: request })
      }
    }).catch((e) => console.warn('[xaim] 执行授权状态读取失败:', e))
    return () => { active = false }
  }, [])

  const save = async (mode: ExecutionMode, gitNetwork: GitNetwork) => {
    if (projectId === undefined) return
    setBusy(true); setError('')
    try {
      const value = await ipc.setExecutionSettings(projectId, mode, gitNetwork)
      if (useProjectStore.getState().currentProject?.id === projectId) { setSettings(value); setProxyMode(value.gitNetwork.mode) }
    } catch (e) { setError(message(e)) } finally { setBusy(false) }
  }
  const revoke = async (scopeId: string) => {
    if (projectId === undefined) return
    setBusy(true); setError('')
    try {
      const value = await ipc.revokeExecutionGrant(projectId, scopeId)
      if (useProjectStore.getState().currentProject?.id === projectId) setSettings(value)
    }
    catch (e) { setError(message(e)) } finally { setBusy(false) }
  }
  const decide = async (decision: PermissionDecision) => {
    if (!pending || deciding) return
    setRespondingTo(pending.id); setError('')
    try {
      await ipc.resolveExecutionPermission(pending.taskId, pending.id, decision)
      // Only the backend event clears this request, after saving its decision.
    } catch (e) {
      setError(message(e))
      setRespondingTo((id) => id === pending.id ? null : id)
    }
  }

  return <div className="execution-permissions">
    <button type="button" className="execution-entry" disabled={projectId === undefined}
      aria-label="项目执行权限" aria-expanded={open} onClick={() => setOpen(!open)}>
      <ShieldCheck size={14} />项目执行权限 · {loading ? '读取中…' : labels[settings.mode]}
    </button>
    {open && !pending && <section className="execution-panel" aria-label="项目执行权限设置">
      <strong>项目执行权限</strong>
      <label>权限模式<select aria-label="权限模式" value={settings.mode} disabled={busy || loading || running}
        onChange={(e) => void save(e.target.value as ExecutionMode, settings.gitNetwork)}>
        <option value="disabled">不执行程序</option><option value="ask">按需询问（建议默认）</option><option value="project">按项目授权</option>
      </select></label>
      <p>按需询问：需要执行时展示具体操作。按项目授权：只有已批准的相同操作自动执行，新增范围仍询问。</p>
      <label>Git 网络<select aria-label="Git 网络" value={proxyMode} disabled={busy || loading || running}
        onChange={(e) => {
          const mode = e.target.value as GitNetwork['mode']
          if (mode === 'custom') setProxyMode(mode)
          else void save(settings.mode, { mode, url: '' })
        }}><option value="system">系统代理 / 环境配置</option><option value="direct">直连</option><option value="custom">自定义代理</option></select></label>
      {proxyMode === 'custom' && <div className="execution-proxy">
        <input aria-label="代理地址" placeholder="http://127.0.0.1:7897" value={proxyUrl} disabled={busy || running} onChange={(e) => setProxyUrl(e.target.value)} />
        <button disabled={busy || running || !proxyUrl.trim()} onClick={() => void save(settings.mode, { mode: 'custom', url: proxyUrl.trim() })}>保存代理</button>
      </div>}
      {proxyMode === 'custom' && <p>输入地址并保存后生效。</p>}
      <p>Git 网络设置仅作用于 XAim 启动的仓库下载进程，不改写全局 Git 配置。系统模式优先保留已有 Git 和环境代理；暂不解析 PAC。</p>
      <strong>已记住的操作 · {settings.grants.length}</strong>
      {settings.grants.length === 0 ? <p>尚未保存授权。执行时可选择“按项目记住”。</p> : <ul className="execution-grants">{settings.grants.map((scope) =>
        <li key={scope.id}><code>{scope.command}</code><span>{scope.location} · {scope.network}</span><p>{scope.impact}</p>
          <button disabled={busy} onClick={() => void revoke(scope.id)}>撤销授权</button></li>)}</ul>}
    </section>}
    {pending && <section role="dialog" aria-label="执行授权" className="execution-request">
      <strong>{pending.purpose}</strong>
      <dl><dt>操作</dt><dd><code>{pending.scope.command}</code></dd><dt>位置</dt><dd>{pending.scope.location}</dd>
        <dt>网络</dt><dd>{pending.scope.network}</dd><dt>影响</dt><dd>{pending.scope.impact}</dd></dl>
      <p>仅允许这项操作及本轮相同范围的重复执行。参数、位置、清单或直接调用的脚本变化时重新询问。</p>
      <div className="execution-actions"><button disabled={deciding} onClick={() => void decide('once')}>允许本轮</button>
        <button disabled={deciding} onClick={() => void decide('remember')}>按项目记住</button>
        <button disabled={deciding} onClick={() => void decide('deny')}>取消</button></div>
    </section>}
    {error && <p role="alert" className="text-xs text-rose-400">{error}</p>}
  </div>
}
