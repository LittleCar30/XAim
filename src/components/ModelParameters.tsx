import { EFFORT_LABELS, reasoningEfforts, supportsThinkingToggle, temperatureEnabled, tokenParameter } from '@/lib/model-profiles'
import type { ModelOptions } from '@/lib/model-profiles'
import type { ModelProfile, ReasoningEffort } from '@/types'

/** One parameter editor for the composer dialog and Settings. */
export default function ModelParameters({ profile, temperature, maxTokens, onTemperature, onMaxTokens, onOptions }: {
  profile: ModelProfile
  temperature: string
  maxTokens: string
  onTemperature: (value: string) => void
  onMaxTokens: (value: string) => void
  onOptions: (patch: Partial<ModelOptions>) => void
}) {
  const efforts = reasoningEfforts(profile)
  const sampling = temperatureEnabled(profile)
  return <div className="model-parameters space-y-3">
    {['openai', 'newapi'].includes(profile.config.provider) && <label className="model-checkbox"><input type="checkbox" checked={profile.supportsReasoning} onChange={(e) => onOptions({ supportsReasoning: e.target.checked, reasoningEffort: null })} />支持思考强度（仅在模型支持时开启）</label>}
    {supportsThinkingToggle(profile.config.provider, profile.config.name) && <label>思考模式<select value={profile.thinking == null ? 'default' : profile.thinking ? 'enabled' : 'disabled'} onChange={(e) => onOptions({ thinking: e.target.value === 'default' ? null : e.target.value === 'enabled', ...(e.target.value === 'disabled' ? { reasoningEffort: null } : {}) })}><option value="default">提供方默认</option><option value="enabled">开启</option><option value="disabled">关闭</option></select></label>}
    {efforts.length > 0 && <label>思考强度<select value={profile.reasoningEffort ?? 'default'} onChange={(e) => onOptions({ reasoningEffort: e.target.value === 'default' ? null : e.target.value as ReasoningEffort })}><option value="default">提供方默认</option>{efforts.map((effort) => <option key={effort} value={effort}>{EFFORT_LABELS[effort]}</option>)}</select></label>}
    <div className="model-form-grid"><label>Temperature<input type="number" min="0" max="2" step="0.1" disabled={!sampling} value={temperature} onChange={(e) => onTemperature(e.target.value)} /></label><label>Max Tokens<input type="number" min="1" placeholder="自动" value={maxTokens} onChange={(e) => onMaxTokens(e.target.value)} /></label></div>
    <p className="text-xs text-slate-400">{sampling ? 'Temperature 控制随机性；较低值更稳定。' : '此推理配置不发送 Temperature，保留值供切换模式后使用。'} 输出上限使用 {tokenParameter(profile)}；留空按模型选择预算，推理输出预算可能包含思考 token。</p>
    {profile.config.provider === 'anthropic' && <p role="status" className="text-xs text-amber-400">Anthropic 原生接入尚未实现；Claude 模型请使用支持该模型的 OpenAI 兼容网关。</p>}
  </div>
}
