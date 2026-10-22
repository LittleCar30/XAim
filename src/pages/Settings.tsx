import { useEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'

import { toMessage } from '@/lib/task-format'
import { ipc } from '@/services/ipc'
import { useConfigStore } from '@/stores/config'
import { useThemeStore } from '@/stores/theme'
import { activeProfile, DEFAULT_MODEL_CONFIG, updateModelProfile } from '@/lib/model-profiles'
import type { ModelOptions } from '@/lib/model-profiles'
import type { ApiProtocol, ModelConfig, ModelProfile } from '@/types'
import ModelManager from '@/components/ModelManager'
import ModelParameters from '@/components/ModelParameters'
import StorageSettings from '@/components/StorageSettings'
import RuntimeSettings from '@/components/RuntimeSettings'
import { savePreference } from '@/services/preferences'
import { Bot, Database, FolderSearch, Palette, ShieldCheck, Workflow } from 'lucide-react'

const CATEGORIES = [
  { id: 'model', label: '模型与连接', description: '配置模型服务、连接与推理参数', icon: Bot },
  { id: 'execution', label: '任务与检查', description: '设置执行额度，了解项目检查与修复', icon: Workflow },
  { id: 'index', label: '项目索引', description: '文件检索与索引偏好', icon: FolderSearch },
  { id: 'safety', label: '安全与确认', description: '查看代码应用与命令执行偏好', icon: ShieldCheck },
  { id: 'appearance', label: '界面与外观', description: '主题和界面语言', icon: Palette },
  { id: 'storage', label: '数据与存储', description: '管理数据目录、备份和工作区保留期限', icon: Database },
] as const
type Category = typeof CATEGORIES[number]['id']

export interface SettingsProps {
  /** 返回工作区（由路由层提供）。 */
  onBack?: () => void
}

// ---------------------------------------------------------------------------
// 表单模型与持久化
// ---------------------------------------------------------------------------

interface SettingsForm {
  model: {
    protocol?: ApiProtocol
    provider: string
    name: string
    apiKey: string
    baseUrl: string
    temperature: string
    maxTokens: string
  }
  index: { ignoreDirs: string; maxFiles: string }
  verify: { enabled: boolean; command: string }
  repair: { maxRounds: string; timeoutSec: string }
  safety: { whitelist: string; confirmPolicy: 'always' | 'auto' | 'readonly' }
  ui: { theme: 'dark' | 'light'; language: 'zh-CN' | 'en' }
}

const DEFAULT_FORM: SettingsForm = {
  model: { provider: DEFAULT_MODEL_CONFIG.provider, name: DEFAULT_MODEL_CONFIG.name, apiKey: '', baseUrl: '', temperature: String(DEFAULT_MODEL_CONFIG.temperature), maxTokens: '' },
  index: { ignoreDirs: 'node_modules\ntarget\ndist', maxFiles: '50000' },
  verify: { enabled: false, command: '' },
  repair: { maxRounds: '2', timeoutSec: '60' },
  safety: { whitelist: '', confirmPolicy: 'always' },
  ui: { theme: 'light', language: 'zh-CN' },
}

const PREFS_KEY = 'xaim.settings-prefs'

function loadPrefs(): Partial<SettingsForm> {
  try {
    const raw = localStorage.getItem(PREFS_KEY)
    return raw ? (JSON.parse(raw) as Partial<SettingsForm>) : {}
  } catch {
    return {}
  }
}

async function savePrefs(form: SettingsForm): Promise<void> {
  await savePreference(
    PREFS_KEY,
    JSON.stringify({
      index: form.index,
      verify: form.verify,
      repair: form.repair,
      safety: form.safety,
      ui: { ...form.ui, theme: useThemeStore.getState().theme },
    }),
  )
}

// ---------------------------------------------------------------------------
// 校验：返回 字段路径 → 错误消息；无错误返回空对象。
// ---------------------------------------------------------------------------

function validate(form: SettingsForm): Record<string, string> {
  const errors: Record<string, string> = {}

  if (!form.model.provider) errors['model.provider'] = '请选择模型提供方'
  if (!form.model.name.trim()) errors['model.name'] = '请填写模型名称'
  if (form.model.provider !== 'ollama' && !form.model.apiKey.trim()) {
    errors['model.apiKey'] = '云端提供方需要填写 API Key（Ollama 可留空）'
  }
  if (form.model.provider === 'newapi' && !form.model.baseUrl.trim()) {
    errors['model.baseUrl'] = 'NewAPI 必须填写 Base URL，例如 https://你的域名/v1'
  }
  if (form.model.baseUrl.trim() !== '' && !/^https?:\/\/\S+$/.test(form.model.baseUrl.trim())) {
    errors['model.baseUrl'] = '必须是 http(s) 地址'
  }
  const temperature = Number(form.model.temperature)
  if (form.model.temperature.trim() === '' || !Number.isFinite(temperature) || temperature < 0 || temperature > 2) {
    errors['model.temperature'] = '取值范围 0 ~ 2'
  }
  if (form.model.maxTokens.trim() !== '') {
    const maxTokens = Number(form.model.maxTokens)
    if (!Number.isInteger(maxTokens) || maxTokens <= 0) errors['model.maxTokens'] = '必须是正整数'
  }
  const maxFiles = Number(form.index.maxFiles)
  if (form.index.maxFiles.trim() === '' || !Number.isInteger(maxFiles) || maxFiles < 1 || maxFiles > 1_000_000) {
    errors['index.maxFiles'] = '取值范围 1 ~ 1000000'
  }
  if (form.verify.enabled && !form.verify.command.trim()) {
    errors['verify.command'] = '启用自动验证时必须填写验证命令'
  }
  const maxRounds = Number(form.repair.maxRounds)
  if (form.repair.maxRounds.trim() === '' || !Number.isInteger(maxRounds) || maxRounds < 0 || maxRounds > 10) {
    errors['repair.maxRounds'] = '取值范围 0 ~ 10'
  }
  const timeoutSec = Number(form.repair.timeoutSec)
  if (form.repair.timeoutSec.trim() === '' || !Number.isInteger(timeoutSec) || timeoutSec < 1) {
    errors['repair.timeoutSec'] = '必须是不小于 1 的整数秒'
  }
  return errors
}

// ---------------------------------------------------------------------------
// 表单 <-> 后端 ModelConfig 转换
// ---------------------------------------------------------------------------

function toModelConfig(form: SettingsForm): ModelConfig {
  return {
    ...(form.model.protocol ? { protocol: ['openai', 'newapi', 'deepseek'].includes(form.model.provider) ? form.model.protocol : 'chat_completions' as const } : {}),
    provider: form.model.provider,
    name: form.model.name.trim(),
    apiKey: form.model.apiKey.trim() === '' ? null : form.model.apiKey,
    baseUrl: form.model.baseUrl.trim() === '' ? null : form.model.baseUrl.trim(),
    temperature: Number(form.model.temperature),
    maxTokens: form.model.maxTokens.trim() === '' ? null : Number(form.model.maxTokens),
  }
}

function fromModelConfig(m: ModelConfig | undefined): SettingsForm['model'] {
  return {
    protocol: m?.protocol,
    provider: m?.provider ?? DEFAULT_FORM.model.provider,
    name: m?.name ?? DEFAULT_FORM.model.name,
    apiKey: m?.apiKey ?? '',
    baseUrl: m?.baseUrl ?? '',
    temperature: String(m?.temperature ?? DEFAULT_FORM.model.temperature),
    maxTokens: m?.maxTokens != null ? String(m.maxTokens) : '',
  }
}

// ---------------------------------------------------------------------------
// 通用小组件
// ---------------------------------------------------------------------------

const inputCls =
  'w-full rounded-md bg-slate-950 px-2.5 py-1.5 text-sm text-slate-100 outline-none ring-1 ring-slate-800 placeholder:text-slate-600 focus:ring-sky-500 disabled:opacity-50'
const inputErrorCls = 'ring-rose-500/70'

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settings-card">
      <h2 className="mb-3 text-sm font-semibold text-slate-200">{title}</h2>
      <div className="space-y-3">{children}</div>
    </section>
  )
}

function Field({
  label,
  error,
  title,
  children,
}: {
  label: string
  error?: string
  title?: string
  children: ReactNode
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs text-slate-400">{label}</span>
      {children}
      {error && <span className="mt-0.5 block text-[11px] text-rose-400">{error}</span>}
      {title && !error && <span className="mt-0.5 block text-[11px] text-slate-600">{title}</span>}
    </label>
  )
}

function textCls(error?: string): string {
  return `${inputCls} ${error ? inputErrorCls : ''}`
}

/** 后端 list_providers 不可用时的兜底清单（与 SUPPORTED_PROVIDERS 一致）。 */
const FALLBACK_PROVIDERS = ['openai', 'deepseek', 'qwen', 'ollama', 'anthropic', 'newapi']

// ---------------------------------------------------------------------------
// 页面
// ---------------------------------------------------------------------------

export default function Settings({ onBack }: SettingsProps) {
  const [category, setCategory] = useState<Category>('model')
  const activeCategory = CATEGORIES.find((item) => item.id === category)!
  const [modelManager, setModelManager] = useState(false)
  const config = useConfigStore((s) => s.config)
  const configBusy = useConfigStore((s) => s.loading)
  const [modelOptionsDraft, setModelOptionsDraft] = useState<ModelOptions>({ supportsReasoning: false, reasoningEffort: null, thinking: null })
  const loadConfig = useConfigStore((s) => s.loadConfig)
  const saveModelConfig = useConfigStore((s) => s.saveModelConfig)
  const testConnection = useConfigStore((s) => s.testConnection)
  const theme = useThemeStore((s) => s.theme)
  const setTheme = useThemeStore((s) => s.setTheme)

  const [form, setForm] = useState<SettingsForm>(() => ({ ...DEFAULT_FORM, ...loadPrefs() }))
  const [errors, setErrors] = useState<Record<string, string>>({})
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loadingConfig, setLoadingConfig] = useState(true)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)

  const [providers, setProviders] = useState<string[]>(FALLBACK_PROVIDERS)
  const [modelOptions, setModelOptions] = useState<string[]>([])
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null)

  const set = <K extends keyof SettingsForm>(group: K, patch: Partial<SettingsForm[K]>) => {
    if (group === 'model') {
      const nextForm = { ...form, model: { ...form.model, ...patch } }
      const next = updateModelProfile(modelDraft, toModelConfig(nextForm))
      setModelOptionsDraft({ supportsReasoning: next.supportsReasoning, reasoningEffort: next.reasoningEffort, thinking: next.thinking })
      setTestResult(null)
    }
    setForm((prev) => ({ ...prev, [group]: { ...prev[group], ...patch } }))
    // 字段修改即清除该组校验错误，避免过期提示。
    setErrors((prev) => {
      const next = { ...prev }
      for (const key of Object.keys(next)) {
        if (key.startsWith(`${group}.`)) delete next[key]
      }
      return next
    })
  }

  // 挂载：拉取后端配置填充模型组，并拉取 provider 清单。
  useEffect(() => {
    let cancelled = false
    ;(async () => {
      try {
        await loadConfig()
      } catch (e) {
        if (!cancelled) setLoadError(toMessage(e))
      } finally {
        if (!cancelled) setLoadingConfig(false)
      }
    })()
    ipc
      .listProviders()
      .then((list) => {
        if (!cancelled && list.length > 0) setProviders(list)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [loadConfig])

  useEffect(() => {
    if (!config) return
    const selected = activeProfile(config)!
    setForm((prev) => ({ ...prev, model: fromModelConfig(selected.config) }))
    setModelOptionsDraft({ supportsReasoning: selected.supportsReasoning, reasoningEffort: selected.reasoningEffort, thinking: selected.thinking })
    setTestResult(null)
    setErrors({})
  }, [config])

  const modelDraft: ModelProfile = updateModelProfile({
    id: activeProfile(config)?.id ?? 'legacy-model', label: activeProfile(config)?.label ?? '当前模型',
    config: toModelConfig(form), ...modelOptionsDraft,
  }, toModelConfig(form))

  const fetchModelOptions = useMemo(
    () => (provider: string) => {
      ipc
        .listModels(provider)
        .then((list) => setModelOptions(list))
        .catch(() => setModelOptions([]))
    },
    [],
  )

  useEffect(() => {
    fetchModelOptions(form.model.provider)
  }, [form.model.provider, fetchModelOptions])

  const handleSave = async () => {
    const errs = validate(form)
    setErrors(errs)
    if (Object.keys(errs).length > 0) {
      const group = Object.keys(errs)[0].split('.')[0]
      setCategory(group === 'verify' || group === 'repair' ? 'execution' : group === 'ui' ? 'appearance' : group as Category)
      return
    }
    setSaveError(null)
    try {
      await savePrefs(form)
    } catch (e) {
      setSaveError(`偏好设置保存失败：${toMessage(e)}`)
      return
    }
    try {
      await saveModelConfig(modelDraft.config, { supportsReasoning: modelDraft.supportsReasoning, reasoningEffort: modelDraft.reasoningEffort, thinking: modelDraft.thinking })
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch (e) {
      setSaveError(`模型配置保存失败：${toMessage(e)}（偏好设置已保存）`)
    }
  }

  const handleTestConnection = async () => {
    const errs = validate(form)
    setErrors(errs)
    if (Object.keys(errs).some((k) => k.startsWith('model.'))) {
      setTestResult({ ok: false, text: '请先修正模型配置中的错误' })
      return
    }
    setTesting(true)
    setTestResult(null)
    try {
      const ok = await testConnection(modelDraft.config, modelDraft.reasoningEffort ?? undefined, modelDraft.thinking, modelDraft.supportsReasoning)
      setTestResult(
        ok
          ? { ok: true, text: '连接成功' }
          : { ok: false, text: '连接失败：服务端返回未通过' },
      )
    } catch (e) {
      setTestResult({ ok: false, text: `连接失败：${toMessage(e)}` })
    } finally {
      setTesting(false)
    }
  }

  const m = form.model
  const mErr = (key: string) => errors[key]

  return (
    <div className="settings-page">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-slate-800 bg-slate-900 px-4">
        {onBack && (
          <button
            type="button"
            onClick={onBack}
            aria-label="返回工作区"
            className="flex items-center gap-1 rounded-md px-2 py-1 text-sm text-slate-400 hover:bg-slate-800 hover:text-slate-100"
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <polyline points="15 18 9 12 15 6" />
            </svg>
            返回
          </button>
        )}
        <h1 className="text-sm font-semibold text-slate-100">设置</h1>
        <div className="ml-auto flex items-center gap-2">
          {saved && <span className="text-xs text-emerald-400">已保存</span>}
          {saveError && (
            <span className="max-w-[28rem] truncate text-xs text-rose-400" title={saveError}>
              {saveError}
            </span>
          )}
          <button
            type="button"
            disabled={loadingConfig || configBusy || !!loadError}
            onClick={() => void handleSave()}
            className="rounded-md bg-sky-500 px-3 py-1.5 text-xs font-medium text-slate-950 hover:bg-sky-400"
          >
            保存全部
          </button>
        </div>
      </header>

      <div className="settings-layout">
        <nav className="settings-navigation" aria-label="设置分类">
          <p className="settings-nav-label">偏好设置</p>
          {CATEGORIES.map(({ id, label, icon: Icon }) => <button key={id} type="button"
            aria-current={category === id ? 'page' : undefined} onClick={() => setCategory(id)}>
            <Icon size={18} aria-hidden="true" /><span>{label}</span>
          </button>)}
          <p className="settings-nav-note">配置你的工作方式</p>
        </nav>
        <main className="settings-main">
        <div className="settings-content">
          <div className="settings-intro"><h2>{activeCategory.label}</h2><p>{activeCategory.description}</p></div>
          {loadError && (
            <p className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-400">
              无法从后端读取配置（{loadError}），以下为本地默认值；连接后端后保存将覆盖。
            </p>
          )}
          {loadingConfig && <p className="text-xs text-slate-500">正在读取配置…</p>}
          <div hidden={category !== 'storage'} className="settings-group"><StorageSettings /></div>
          <div hidden={category !== 'execution'} className="settings-group">
            <RuntimeSettings />
            <Section title="项目检查说明">
              <p className="settings-help">执行权限统一管理测试、构建、依赖安装和开发服务。推荐选择“确认后执行”；只分析模式不会运行程序，自动执行模式只放行已批准过的相同操作。</p>
              <p className="settings-help">关闭执行权限仍能生成代码，检查会标记为无法验证。检查失败时会尝试修复，最多三轮；纯分析任务无需代码检查。已保存的授权可查看和撤销，应用代码仍需单独确认。</p>
              <p className="settings-help">项目脚本以当前用户权限运行，可以访问本机和网络；请在信任项目时启用。</p>
            </Section>
          </div>

          {/* 1. 模型 */}
          <div hidden={category !== 'model'} className="settings-group">
          <Section title="模型">
            <fieldset className="min-w-0 space-y-3" disabled={loadingConfig || configBusy || !!loadError}>
              <button type="button" onClick={() => setModelManager(true)} disabled={loadingConfig || !!loadError} className="rounded-md border border-slate-700 px-3 py-1.5 text-xs disabled:opacity-50">管理模型</button>
              <Field label="提供方" error={mErr('model.provider')}>
                <input
                  list="provider-options"
                  value={m.provider}
                  onChange={(e) => {
                    set('model', { provider: e.target.value })
                  }}
                  onBlur={() => fetchModelOptions(m.provider)}
                  className={textCls(mErr('model.provider'))}
                />
                <datalist id="provider-options">
                  {providers.map((p) => (
                    <option key={p} value={p} />
                  ))}
                </datalist>
              </Field>
              <Field label="模型名称" error={mErr('model.name')} title="可选建议来自后端 list_models">
                <input
                  list="model-options"
                  value={m.name}
                  onChange={(e) => set('model', { name: e.target.value })}
                  className={textCls(mErr('model.name'))}
                />
                <datalist id="model-options">
                  {modelOptions.map((n) => (
                    <option key={n} value={n} />
                  ))}
                </datalist>
              </Field>
              <Field
                label="API Key"
                error={mErr('model.apiKey')}
                title={m.provider === 'ollama' ? 'Ollama 等本地服务可留空' : undefined}
              >
                <input
                  type="password"
                  value={m.apiKey}
                  onChange={(e) => set('model', { apiKey: e.target.value })}
                  className={textCls(mErr('model.apiKey'))}
                />
              </Field>
              <Field label="Base URL" error={mErr('model.baseUrl')} title="留空使用提供方默认地址">
                <input
                  value={m.baseUrl}
                  onChange={(e) => set('model', { baseUrl: e.target.value })}
                  placeholder="https://api.example.com/v1"
                  className={textCls(mErr('model.baseUrl'))}
                />
              </Field>
              <ModelParameters onProtocol={(protocol) => set('model', { protocol })} profile={modelDraft} temperature={m.temperature} maxTokens={m.maxTokens}
                onTemperature={(value) => set('model', { temperature: value })} onMaxTokens={(value) => set('model', { maxTokens: value })}
                onOptions={(patch) => setModelOptionsDraft((prev) => ({ ...prev, ...patch }))} />
              {mErr('model.temperature') && <p role="alert">{mErr('model.temperature')}</p>}
              {mErr('model.maxTokens') && <p role="alert">{mErr('model.maxTokens')}</p>}
              <div className="flex items-center gap-2 pt-1">
                <button
                  type="button"
                  onClick={() => void handleTestConnection()}
                  disabled={testing}
                  className="rounded-md border border-slate-700 px-3 py-1.5 text-xs text-slate-200 hover:bg-slate-800 disabled:opacity-50"
                >
                  {testing ? '测试中…' : '测试连接'}
                </button>
                {testResult && (
                  <span className={`text-xs ${testResult.ok ? 'text-emerald-400' : 'text-rose-400'}`}>
                    {testResult.text}
                  </span>
                )}
              </div>
            </fieldset>
          </Section>
          </div>

          {/* 2. 索引 */}
          <div hidden={category !== 'index'} className="settings-group">
          <p className="settings-preference-note">以下选项目前仅保存界面偏好，尚未接入执行内核。</p>
          <Section title="索引">
            <Field
              label="忽略目录"
              title="每行一个目录名，也可用逗号分隔；目前仅保存偏好"
            >
              <textarea
                rows={4}
                value={form.index.ignoreDirs}
                onChange={(e) => set('index', { ignoreDirs: e.target.value })}
                className={`${inputCls} resize-none font-mono`}
              />
            </Field>
            <Field label="最大文件数" error={mErr('index.maxFiles')}>
              <input
                type="number"
                min="1"
                value={form.index.maxFiles}
                onChange={(e) => set('index', { maxFiles: e.target.value })}
                className={textCls(mErr('index.maxFiles'))}
              />
            </Field>
          </Section>
          </div>

          {/* 3. 验证 */}
          <div hidden={category !== 'execution'} className="settings-group">
          <p className="settings-preference-note">以下验证与修复选项目前仅保存界面偏好。实际项目检查由每次任务的勾选授权决定。</p>
          <Section title="验证">
            <label className="flex items-center gap-2 text-sm text-slate-300">
              <input
                type="checkbox"
                checked={form.verify.enabled}
                onChange={(e) => set('verify', { enabled: e.target.checked })}
                className="h-4 w-4 rounded bg-slate-950 ring-1 ring-slate-700"
              />
              生成补丁后自动运行验证
            </label>
            <Field
              label="验证命令"
              error={mErr('verify.command')}
              title={form.verify.enabled ? undefined : '未启用自动验证时无需填写'}
            >
              <input
                value={form.verify.command}
                onChange={(e) => set('verify', { command: e.target.value })}
                placeholder="npm test"
                className={textCls(mErr('verify.command'))}
              />
            </Field>
          </Section>

          {/* 4. 修复 */}
          <Section title="修复">
            <div className="grid grid-cols-2 gap-3">
              <Field label="最大修复轮数" error={mErr('repair.maxRounds')}>
                <input
                  type="number"
                  min="0"
                  max="10"
                  value={form.repair.maxRounds}
                  onChange={(e) => set('repair', { maxRounds: e.target.value })}
                  className={textCls(mErr('repair.maxRounds'))}
                />
              </Field>
              <Field label="单步超时（秒）" error={mErr('repair.timeoutSec')}>
                <input
                  type="number"
                  min="1"
                  value={form.repair.timeoutSec}
                  onChange={(e) => set('repair', { timeoutSec: e.target.value })}
                  className={textCls(mErr('repair.timeoutSec'))}
                />
              </Field>
            </div>
          </Section>

          {/* 5. 安全 */}
          </div>
          <div hidden={category !== 'safety'} className="settings-group">
          <p className="settings-preference-note">以下选项目前仅保存界面偏好。当前代码变更均需手动应用，模型不能执行任意命令。</p>
          <Section title="安全">
            <Field label="命令白名单" title="每行一条允许执行的命令前缀；留空表示不限制">
              <textarea
                rows={3}
                value={form.safety.whitelist}
                onChange={(e) => set('safety', { whitelist: e.target.value })}
                placeholder={'cargo\ngit'}
                className={`${inputCls} resize-none font-mono`}
              />
            </Field>
            <Field label="确认策略">
              <select
                value={form.safety.confirmPolicy}
                onChange={(e) =>
                  set('safety', { confirmPolicy: e.target.value as SettingsForm['safety']['confirmPolicy'] })
                }
                className={inputCls}
              >
                <option value="always">每次应用前确认</option>
                <option value="auto">自动应用通过的补丁</option>
                <option value="readonly">只读模式（不应用补丁）</option>
              </select>
            </Field>
          </Section>
          </div>

          {/* 6. 界面 */}
          <div hidden={category !== 'appearance'} className="settings-group">
          <Section title="界面">
            <div className="grid grid-cols-2 gap-3">
              <Field label="主题">
                <select
                  value={theme}
                  onChange={(e) => {
                    const next = e.target.value as SettingsForm['ui']['theme']
                    setTheme(next)
                    set('ui', { theme: next })
                  }}
                  className={inputCls}
                >
                  <option value="light">浅色</option>
                  <option value="dark">深色</option>
                </select>
              </Field>
              <Field label="语言" title="英文界面将在后续版本提供">
                <select
                  value={form.ui.language}
                  onChange={(e) => set('ui', { language: e.target.value as SettingsForm['ui']['language'] })}
                  className={inputCls}
                >
                  <option value="zh-CN">简体中文</option>
                  <option value="en" disabled>
                    English（后续支持）
                  </option>
                </select>
              </Field>
            </div>
          </Section>
          </div>

          <p className="pb-2 text-center text-[11px] text-slate-600">
            模型和偏好通过“保存全部”保存；任务设置与存储操作在各自分类中保存，主题即时生效。
          </p>
        </div>
        </main>
      </div>
      {modelManager && <ModelManager onClose={() => setModelManager(false)} />}
    </div>
  )
}
