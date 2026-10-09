import { Check, ChevronDown, ChevronRight, Circle, Loader2, X, CircleHelp } from 'lucide-react'
import type { XaimEvent } from '@/types'
import ActionBar from '@/components/ActionBar'
import DiffView from '@/components/DiffView'
import { TASK_STATUS_LABELS, taskStatusBadgeCls } from '@/lib/task-format'
import { useConversationStore } from '@/stores/conversation'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { useUiStore } from '@/stores/ui'

export default function TaskPanel() {
  const currentTask = useTaskStore((s) => s.currentTask)
  const livePatches = useTaskStore((s) => s.patches)
  const liveSteps = useTaskStore((s) => s.steps)
  const liveVerifies = useTaskStore((s) => s.verifies)
  const events = useTaskStore((s) => s.events)
  const project = useProjectStore((s) => s.currentProject)
  const { selectedId, record, loading, error } = useConversationStore()
  const historical = selectedId !== null
  const task = historical ? record?.task : currentTask
  const patches = historical ? record?.patches ?? [] : livePatches
  const rawSteps = historical ? record?.steps ?? [] : liveSteps
  const savedEvents = historical ? rawSteps.filter((step) => step.stepType === 'event').flatMap((step): XaimEvent[] => {
    try { return [JSON.parse(step.content) as XaimEvent] } catch { return [] }
  }) : events
  const steps = historical ? rawSteps.flatMap((step) => {
    if (step.stepType !== 'event') return [step]
    try {
      const event = JSON.parse(step.content) as XaimEvent
      return event.type === 'step' ? [{ ...step, stepType: event.name, content: event.status }] : []
    } catch { return [] }
  }) : liveSteps
  const verifies = historical ? record?.verifies ?? [] : liveVerifies
  if (loading || error) return <div className="conversation-scroll"><div className="conversation-content"><p role={error ? 'alert' : 'status'}>{error ? `无法读取历史对话：${error}` : '正在读取历史对话…'}</p></div></div>
  if (!task) return <div className="conversation-scroll"><div className="conversation-empty"><img src="/xaim.svg" alt="XAim" width="44" height="44" /><h1>新对话</h1><p>{project?.name ?? '未打开项目'}</p></div></div>
  const running = !['waitingConfirm', 'idle', 'done', 'applied', 'failed', 'cancelled', 'rolledBack'].includes(task.status)
  // Coalesce running/done event pairs into one row per stage.
  const stages = [...new Map(steps.filter((step) =>
    step.stepType !== 'diagnostic' && !(['intent', 'plan'].includes(step.stepType) && step.content.trim().startsWith('{')),
  ).map((step) => [step.stepType, step])).values()].map((step) => ({
    ...step,
    content: step.content === 'running' && ['failed', 'cancelled'].includes(task.status) ? task.status : step.content,
  }))
  const latest = stages.at(-1)
  const passed = verifies.filter((verify) => verify.passed).length
  const errorEvent = [...savedEvents].reverse().find((event) => event.type === 'error' && event.taskId === task.id)
  const message = [...savedEvents].reverse().find((event) => event.type === 'message' && event.taskId === task.id)
  const textEvents = savedEvents.filter((event): event is Extract<XaimEvent, { type: 'modelText' }> =>
    event.type === 'modelText' && event.taskId === task.id,
  )
  const latestText = textEvents.at(-1)
  const responseText = latestText ? textEvents.filter((event) => event.round === latestText.round).map((event) => event.delta).join('') : ''
  const interruptedText = latestText?.status === 'interrupted' || latestText && ['failed', 'cancelled'].includes(task.status) && latestText.status !== 'complete'
  const unavailable = verifies.filter((verify) => verify.status === 'unavailable').length
  const summary = task.status === 'waitingConfirm' ? `已生成 ${patches.length} 个代码变更，等待你确认后应用。`
    : task.status === 'done' || task.status === 'applied' ? `任务已完成${patches.some((patch) => patch.applied) ? `，已应用 ${patches.filter((patch) => patch.applied).length} 个代码变更` : ''}。`
      : task.status === 'rolledBack' ? '本次代码变更已回滚。' : task.status === 'cancelled' ? '本次任务已取消。' : task.status === 'failed' ? '任务执行失败。' : '正在处理你的请求。'
  return <div className="conversation-scroll" role="log" aria-label="对话记录"><div className="conversation-content" key={task.id}>
    <article className="user-message"><span className="message-label">你</span><p>{task.prompt}</p></article>
    <article className="agent-message">
      <header className="agent-header"><img src="/xaim.svg" width="28" height="28" alt="" /><strong>XAim</strong><span className={`task-status ${taskStatusBadgeCls(task.status)}`}>{TASK_STATUS_LABELS[task.status]}</span></header>
      <p className="agent-summary">{summary}</p>
      {message?.type === 'message' && <p className="agent-summary whitespace-pre-wrap">{message.content}</p>}
      {responseText && (running || interruptedText || historical && !message) && <div aria-label="模型回复">
        <p className="agent-summary whitespace-pre-wrap">{responseText}</p>
        {interruptedText && <p className="historical-note">回复已中断，内容尚未完成。</p>}
      </div>}
      {errorEvent?.type === 'error' && <p className="message-error">{errorEvent.message}</p>}
      {stages.length > 0 && <details className="execution-details" open={running || task.status === 'failed' || undefined}>
        <summary>{running ? <Loader2 size={15} className="animate-spin" /> : task.status === 'failed' ? <X size={15} className="text-rose-400" /> : task.status === 'cancelled' ? <Circle size={15} /> : <Check size={15} />}<span>{running ? latest?.stepType : '执行过程'}</span><span className="execution-count">{stages.length} 个步骤</span><ChevronDown size={15} /></summary>
        <ol aria-label="任务步骤">{stages.map((step) => <li key={step.stepType}>
          {/done|完成|成功/.test(step.content) ? <Check size={14} className="text-emerald-400" /> : step.content === 'failed' ? <X size={14} className="text-rose-400" /> : <Circle size={12} />}
          <span>{step.stepType}</span><span className="step-content">{({ done: '完成', running: '进行中', failed: '失败', cancelled: '已取消' } as Record<string, string>)[step.content] ?? step.content}</span>
        </li>)}</ol>
      </details>}
      {patches.length > 0 && <section className="message-changes" aria-label="代码变更"><div className="result-heading"><h2>代码变更</h2><span>{new Set(patches.map((patch) => patch.filePath)).size} 个文件</span></div><DiffView patches={patches} compact /></section>}
      {verifies.length > 0 && <div className="verification-summary">{unavailable ? <CircleHelp size={15} className="text-slate-500" /> : <Check size={15} className={passed === verifies.length ? 'text-emerald-400' : 'text-rose-400'} />}<span>验证 · {passed}/{verifies.length} 通过{unavailable ? ` · ${unavailable} 项无法验证` : ''}</span><button onClick={() => useUiStore.setState({ rightPanelOpen: true, previewPath: null, detailTab: 'verify' })}>查看详情<ChevronRight size={14} /></button></div>}
      {!historical && patches.length > 0 && !['failed', 'cancelled'].includes(task.status) && <ActionBar />}
      {historical && <p className="historical-note">历史任务 · {new Date(task.createdAt).toLocaleString('zh-CN')}{verifies.length === 0 ? ' · 未保存验证输出' : ''}</p>}
    </article>
  </div></div>
}
