import { useEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'

import { toMessage } from '@/lib/task-format'
import { ipc } from '@/services/ipc'
import { useConfigStore } from '@/stores/config'
import { useThemeStore } from '@/stores/theme'
import type { ModelConfig } from '@/types'
import ModelManager from '@/components/ModelManager'

export interface SettingsProps {
  /** 返回工作区（由路由层提供）。 */
  onBack?: () => void
}

// ---------------------------------------------------------------------------
// 表单模型与持久化
// ---------------------------------------------------------------------------

interface SettingsForm {
  model: {
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
  model: { provider: 'deepseek', name: 'deepseek-chat', apiKey: '', baseUrl: '', temperature: '0.7', maxTokens: '' },
  index: { ignoreDirs: 'node_modules\ntarget\ndist', maxFiles: '50000' },
  verify: { enabled: true, command: '' },
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

function savePrefs(form: SettingsForm): void {
  localStorage.setItem(
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
  if (form.model.baseUrl.trim() !== '' && !/^https?:\/\/\S+$/.test(form.model.baseUrl.trim())) {
    errors['model.baseUrl'] = '必须是 http(s) 地址'
  }
  const temperature = Number(form.model.temperature)
  if (form.model.temperature.trim() === '' || Number.isNaN(temperature) || temperature < 0 || temperature > 2) {
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
    <section className="rounded-lg border border-slate-800 bg-slate-900/50 p-4">
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
const FALLBACK_PROVIDERS = ['openai', 'deepseek', 'qwen', 'ollama', 'anthropic']

// ---------------------------------------------------------------------------
// 页面
// ---------------------------------------------------------------------------

export default function Settings({ onBack }: SettingsProps) {
  const [modelManager, setModelManager] = useState(false)
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
        if (!cancelled) setForm((prev) => ({ ...prev, model: fromModelConfig(useConfigStore.getState().config?.model) }))
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
    if (Object.keys(errs).length > 0) return
    setSaveError(null)
    // 本地偏好先落盘（必定成功），模型配置再走后端；后者失败单独报错。
    savePrefs(form)
    try {
      await saveModelConfig(toModelConfig(form))
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch (e) {
      setSaveError(`模型配置保存失败：${toMessage(e)}（其余设置已保存到本地）`)
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
      const ok = await testConnection(toModelConfig(form))
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
    <div className="flex h-screen flex-col bg-slate-950 text-slate-100">
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
            onClick={() => void handleSave()}
            className="rounded-md bg-sky-500 px-3 py-1.5 text-xs font-medium text-slate-950 hover:bg-sky-400"
          >
            保存全部
          </button>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-auto">
        <div className="mx-auto max-w-2xl space-y-4 p-4">
          {loadError && (
            <p className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-400">
              无法从后端读取配置（{loadError}），以下为本地默认值；连接后端后保存将覆盖。
            </p>
          )}
          {loadingConfig && <p className="text-xs text-slate-500">正在读取配置…</p>}

          {/* 1. 模型 */}
          <Section title="模型">
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
            <div className="grid grid-cols-2 gap-3">
              <Field label="Temperature" error={mErr('model.temperature')}>
                <input
                  type="number"
                  step="0.1"
                  min="0"
                  max="2"
                  value={m.temperature}
                  onChange={(e) => set('model', { temperature: e.target.value })}
                  className={textCls(mErr('model.temperature'))}
                />
              </Field>
              <Field label="Max Tokens" error={mErr('model.maxTokens')} title="留空使用默认值">
                <input
                  type="number"
                  min="1"
                  value={m.maxTokens}
                  onChange={(e) => set('model', { maxTokens: e.target.value })}
                  className={textCls(mErr('model.maxTokens'))}
                />
              </Field>
            </div>
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
          </Section>

          {/* 2. 索引 */}
          <Section title="索引">
            <Field
              label="忽略目录"
              title="每行一个目录名，也可用逗号分隔；保存后下次打开项目生效"
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

          {/* 3. 验证 */}
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

          {/* 6. 界面 */}
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

          <p className="pb-2 text-center text-[11px] text-slate-600">
            模型配置保存到后端；主题即时保存；索引 / 验证 / 修复 / 安全 / 语言为本地偏好。
          </p>
        </div>
      </div>
      {modelManager && <ModelManager onClose={() => { setModelManager(false); setForm((prev) => ({ ...prev, model: fromModelConfig(useConfigStore.getState().modelConfig ?? undefined) })) }} />}
    </div>
  )
}
