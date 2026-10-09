import { useEffect, useRef, useState } from 'react'
import { Check, ChevronDown, Settings2 } from 'lucide-react'
import ModelManager from '@/components/ModelManager'
import { activeProfile, EFFORT_LABELS, modelProfiles, reasoningEfforts } from '@/lib/model-profiles'
import { toMessage } from '@/lib/task-format'
import { useConfigStore } from '@/stores/config'
import type { ReasoningEffort } from '@/types'

export default function ModelPicker({ disabled = false }: { disabled?: boolean }) {
  const config = useConfigStore((s) => s.config)
  const model = useConfigStore((s) => s.modelConfig)
  const loading = useConfigStore((s) => s.loading)
  const [open, setOpen] = useState<'model' | 'effort' | null>(null)
  const [manager, setManager] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const rootRef = useRef<HTMLDivElement>(null)
  const active = activeProfile(config)
  const efforts = active ? reasoningEfforts(active) : []
  const effortSupported = efforts.length > 0
  useEffect(() => {
    if (!useConfigStore.getState().config) void useConfigStore.getState().loadConfig().catch((e) => setError(toMessage(e)))
  }, [])
  useEffect(() => {
    if (!open) return
    const closeOutside = (e: PointerEvent) => { if (!rootRef.current?.contains(e.target as Node)) setOpen(null) }
    const escape = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(null) }
    document.addEventListener('pointerdown', closeOutside); document.addEventListener('keydown', escape)
    return () => { document.removeEventListener('pointerdown', closeOutside); document.removeEventListener('keydown', escape) }
  }, [open])
  const select = async (id: string, effort?: ReasoningEffort | null) => {
    setError(null)
    try { await useConfigStore.getState().selectModel(id, effort); setOpen(null) } catch (e) { setError(toMessage(e)) }
  }
  return <div className="composer-model-controls" ref={rootRef}>
    <button className="composer-model-button" aria-label="选择模型" aria-haspopup="menu" aria-expanded={open === 'model'} title={active?.config.name ?? model?.name ?? '选择模型'} disabled={disabled || loading} onClick={() => setOpen(open === 'model' ? null : 'model')}><span>{active?.label ?? model?.name ?? '选择模型'}</span><ChevronDown size={12} /></button>
    <button className="composer-effort-button" aria-label="选择思考强度" aria-haspopup="menu" aria-expanded={open === 'effort'} disabled={disabled || loading} onClick={() => setOpen(open === 'effort' ? null : 'effort')} title="思考强度">{active?.thinking === false ? '思考关闭' : active?.reasoningEffort ? EFFORT_LABELS[active.reasoningEffort] : '默认'}<ChevronDown size={12} /></button>
    {open === 'model' && <div className="composer-model-menu" role="menu" aria-label="模型选择">{modelProfiles(config).map((p) => <button role="menuitemradio" aria-checked={p.id === active?.id} key={p.id} disabled={loading} onClick={() => void select(p.id)}><span><strong>{p.label}</strong><small>{p.config.provider} · {p.config.name}</small></span>{p.id === active?.id && <Check size={15} />}</button>)}<button role="menuitem" className="manage-models" onClick={() => { setOpen(null); setManager(true) }} disabled={!config}><Settings2 size={15} />管理模型</button></div>}
    {open === 'effort' && <div className="composer-model-menu effort-menu" role="menu" aria-label="思考强度"><button role="menuitemradio" aria-checked={!active?.reasoningEffort} onClick={() => active && void select(active.id, null)}>默认{!active?.reasoningEffort && <Check size={14} />}</button>{(effortSupported ? efforts : ['low', 'medium', 'high'] as ReasoningEffort[]).map((effort) => <button key={effort} role="menuitemradio" aria-checked={active?.reasoningEffort === effort} disabled={!effortSupported || loading} onClick={() => active && void select(active.id, effort)}>{EFFORT_LABELS[effort]}{active?.reasoningEffort === effort && <Check size={14} />}</button>)}{!effortSupported && <p>此模型使用默认强度</p>}</div>}
    {error && <span role="alert" className="model-picker-error" title={error}>{error}</span>}
    {manager && <ModelManager onClose={() => setManager(false)} />}
  </div>
}
