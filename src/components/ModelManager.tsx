import { useEffect, useRef, useState } from 'react'
import { Check, Plus, Trash2, X } from 'lucide-react'
import { activeProfile, DEFAULT_MODEL_CONFIG, modelProfiles, updateModelProfile } from '@/lib/model-profiles'
import { toMessage } from '@/lib/task-format'
import { ipc } from '@/services/ipc'
import { useConfigStore } from '@/stores/config'
import ModelParameters from '@/components/ModelParameters'
import type { ModelProfile } from '@/types'

const newProfile = (): ModelProfile => ({ id: crypto.randomUUID(), label: '', config: {
  ...DEFAULT_MODEL_CONFIG, provider: 'openai', name: '',
}, supportsReasoning: false, reasoningEffort: null })

export default function ModelManager({ onClose }: { onClose: () => void }) {
  const config = useConfigStore((s) => s.config)
  const configLoading = useConfigStore((s) => s.loading)
  const profiles = modelProfiles(config)
  const active = activeProfile(config)
  const [draft, setDraft] = useState<ModelProfile>(() => active ? structuredClone(active) : newProfile())
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [options, setOptions] = useState<string[]>([])
  const dialogRef = useRef<HTMLDialogElement>(null)
  useEffect(() => { dialogRef.current?.showModal() }, [])
  useEffect(() => {
    let disposed = false
    ipc.listModels(draft.config.provider).then((list) => { if (!disposed) setOptions(list) }).catch(() => { if (!disposed) setOptions([]) })
    return () => { disposed = true }
  }, [draft.config.provider])
  const update = (patch: Partial<ModelProfile['config']>) => {
    setDraft((d) => {
      const next = updateModelProfile(d, { ...d.config, ...patch })
      return next
    })
    setError(null); setResult(null)
  }
  const validate = () => {
    if (!draft.label.trim() || !draft.config.name.trim()) return '请填写显示名称和模型名称'
    if (draft.config.provider !== 'ollama' && !draft.config.apiKey?.trim()) return '请填写 API Key'
    if (draft.config.provider === 'newapi' && !draft.config.baseUrl?.trim()) return 'NewAPI 必须填写 Base URL，例如 https://你的域名/v1'
    if (draft.config.baseUrl && !/^https?:\/\/\S+$/.test(draft.config.baseUrl)) return 'Base URL 必须是 http(s) 地址'
    if (!Number.isFinite(draft.config.temperature) || draft.config.temperature < 0 || draft.config.temperature > 2) return 'Temperature 范围为 0 到 2'
    if (draft.config.maxTokens !== null && (!Number.isInteger(draft.config.maxTokens) || draft.config.maxTokens <= 0)) return 'Max Tokens 必须为正整数'
    return null
  }
  const perform = async (action: () => Promise<void>) => {
    setBusy(true); setError(null); setResult(null)
    try { await action() } catch (e) { setError(toMessage(e)) } finally { setBusy(false) }
  }
  const save = async (activate = false) => {
    const message = validate()
    if (message) { setError(message); return }
    await perform(async () => {
      const next = { ...draft, label: draft.label.trim(), config: { ...draft.config, name: draft.config.name.trim() } }
      const list = profiles.some((p) => p.id === next.id) ? profiles.map((p) => p.id === next.id ? next : p) : [...profiles, next]
      await useConfigStore.getState().saveProfiles(list, activate ? next.id : active?.id ?? next.id)
      setDraft(next)
      setResult(activate ? '已保存并设为当前模型' : '已保存')
    })
  }
  const remove = () => perform(async () => {
    const list = profiles.filter((p) => p.id !== draft.id)
    if (!list.length) throw new Error('至少保留一个模型')
    const nextId = active?.id === draft.id ? list[0].id : active!.id
    await useConfigStore.getState().saveProfiles(list, nextId)
    setDraft(structuredClone(list.find((p) => p.id === nextId)!))
    setResult('已删除')
  })
  const activate = () => save(true)
  return <dialog ref={dialogRef} className="model-manager" aria-labelledby="model-manager-title" onCancel={onClose} onClick={(e) => { if (e.target === e.currentTarget) onClose() }}>
    <div className="model-manager-content"><header><h2 id="model-manager-title">管理模型</h2><button className="icon-button" aria-label="关闭模型管理" title="关闭" onClick={onClose}><X size={18} /></button></header>
      <div className="model-manager-body"><nav aria-label="已保存模型">
        {profiles.map((p) => <button key={p.id} disabled={busy || configLoading} className={draft.id === p.id ? 'selected' : ''} onClick={() => { setDraft(structuredClone(p)); setError(null); setResult(null) }}><span>{p.label}</span>{active?.id === p.id && <Check size={14} />}<small>{p.config.provider} · {p.config.name}</small></button>)}
        <button className="add-model" disabled={busy || configLoading} onClick={() => { setDraft(newProfile()); setError(null); setResult(null) }}><Plus size={15} />添加模型</button>
      </nav><form onSubmit={(e) => { e.preventDefault(); void save() }}>
        <fieldset disabled={busy || configLoading}>
          <label>显示名称<input value={draft.label} onChange={(e) => setDraft({ ...draft, label: e.target.value })} placeholder="例如：本地编码模型" required /></label>
          <div className="model-form-grid"><label>提供方<select value={draft.config.provider} onChange={(e) => update({ provider: e.target.value })}>{['openai', 'deepseek', 'qwen', 'ollama', 'anthropic', 'newapi'].map((p) => <option key={p} value={p}>{p === 'newapi' ? 'newapi（OpenAI 兼容）' : p}</option>)}</select></label>
          <label>模型名称<input list="managed-model-options" value={draft.config.name} onChange={(e) => update({ name: e.target.value })} required /><datalist id="managed-model-options">{options.map((name) => <option key={name} value={name} />)}</datalist></label></div>
          <label>API Key<input type="password" autoComplete="off" value={draft.config.apiKey ?? ''} onChange={(e) => update({ apiKey: e.target.value || null })} placeholder="填写裸 token，不要加 Bearer" /></label>
          <label>Base URL<input type="url" value={draft.config.baseUrl ?? ''} onChange={(e) => update({ baseUrl: e.target.value || null })} placeholder={draft.config.provider === 'newapi' ? '例如：https://你的域名/v1' : '使用提供方默认地址'} /></label>
          <ModelParameters onProtocol={(protocol) => update({ protocol })} profile={draft} temperature={String(draft.config.temperature)} maxTokens={draft.config.maxTokens == null ? '' : String(draft.config.maxTokens)}
            onTemperature={(value) => update({ temperature: Number(value) })} onMaxTokens={(value) => update({ maxTokens: value ? Number(value) : null })}
            onOptions={(patch) => { setDraft((d) => ({ ...d, ...patch })); setResult(null); setError(null) }} />
        </fieldset>
        {error && <p role="alert" className="model-error">{error}</p>}{result && <p role="status" className="model-result">{result}</p>}
        <footer><button type="button" className="icon-button" title="删除模型" aria-label="删除模型" disabled={busy || configLoading || profiles.length <= 1 || !profiles.some((p) => p.id === draft.id)} onClick={() => void remove()}><Trash2 size={16} /></button><button type="button" disabled={busy || configLoading} onClick={() => { const message = validate(); if (message) { setError(message); return } void perform(async () => { const ok = await useConfigStore.getState().testConnection(draft.config, draft.reasoningEffort ?? undefined, draft.thinking, draft.supportsReasoning); setResult(ok ? '连接成功' : '连接失败') }) }}>测试连接</button>{active?.id !== draft.id && profiles.some((p) => p.id === draft.id) && <button type="button" disabled={busy || configLoading} onClick={() => void activate()}>设为当前</button>}<button type="submit" className="primary" disabled={busy || configLoading}>{busy ? '处理中…' : '保存模型'}</button></footer>
      </form></div>
    </div>
  </dialog>
}
