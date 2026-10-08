import { useEffect, useRef, useState } from 'react'
import type { KeyboardEvent as ReactKeyboardEvent } from 'react'

import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import type { TaskStatus } from '@/types'
import { ArrowUp, Loader2, Square } from 'lucide-react'
import { useConversationStore } from '@/stores/conversation'
import { useConfigStore } from '@/stores/config'
import ModelPicker from '@/components/ModelPicker'

const HISTORY_KEY = 'xaim.prompt-history'
const HISTORY_LIMIT = 50
const MAX_INPUT_HEIGHT = 160

/** 这些状态表示任务已结束，输入栏恢复可用；其余状态（含 waitingConfirm）视为运行中。 */
const TERMINAL_STATUSES: TaskStatus[] = [
  'idle',
  'applied',
  'done',
  'failed',
  'rolledBack',
  'cancelled',
]

function toMessage(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  return String(e)
}

function loadHistory(): string[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    const parsed = raw ? JSON.parse(raw) : []
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
}

/** 底部输入栏：多行输入、Enter 发送（Shift+Enter 换行）、↑↓ 历史召回、任务运行中禁用。 */
export default function InputBar() {
  const currentProject = useProjectStore((s) => s.currentProject)
  const currentTask = useTaskStore((s) => s.currentTask)
  const setCurrentTask = useTaskStore((s) => s.setCurrentTask)
  const configLoading = useConfigStore((s) => s.loading)
  const awaitingConfirmation = currentTask?.status === 'waitingConfirm'

  const [value, setValue] = useState('')
  const [history, setHistory] = useState<string[]>(loadHistory)
  const [historyIndex, setHistoryIndex] = useState<number | null>(null)
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const textareaRef = useRef<HTMLTextAreaElement | null>(null)

  const running = currentTask !== null && !TERMINAL_STATUSES.includes(currentTask.status)
  const disabled = running || sending || configLoading || currentProject === null
  const canSend = !disabled && value.trim() !== ''

  // 多行自适应高度：随内容增长，封顶后内部滚动。
  useEffect(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, MAX_INPUT_HEIGHT)}px`
  }, [value])

  const send = async () => {
    const project = currentProject
    const prompt = value.trim()
    if (sending || running || project === null || prompt === '') return
    setSending(true)
    setError(null)
    try {
      useConversationStore.getState().archiveCurrent()
      await useConversationStore.getState().select(null)
      const taskHistory = useTaskStore.getState().history
      useTaskStore.getState().reset()
      useTaskStore.setState({ history: taskHistory })
      setCurrentTask({ id: -1, projectId: project.id, prompt, status: 'parsing', intent: null, plan: null,
        createdAt: Date.now(), finishedAt: null, durationMs: null, tokenInput: 0, tokenOutput: 0, cost: null })
      const taskId = await ipc.runTask(project.id, prompt)
      const [task, patches] = await Promise.all([ipc.getTask(taskId), ipc.listPatches(taskId)])
      // Event patches have temporary IDs; actions need the persisted database IDs.
      useTaskStore.setState({ patches })
      setCurrentTask(task)
      const next =
        history[history.length - 1] === prompt ? history : [...history, prompt].slice(-HISTORY_LIMIT)
      setHistory(next)
      setHistoryIndex(null)
      setValue('')
      try {
        localStorage.setItem(HISTORY_KEY, JSON.stringify(next))
      } catch {
        // 持久化失败不影响发送
      }
    } catch (e) {
      setError(toMessage(e))
      const task = useTaskStore.getState().currentTask
      if (task) setCurrentTask({ ...task, status: 'failed', finishedAt: Date.now() })
    } finally {
      setSending(false)
    }
  }

  const recall = (index: number) => {
    setHistoryIndex(index)
    setValue(history[index])
    // 召回后把光标移到末尾，方便直接编辑
    requestAnimationFrame(() => {
      const el = textareaRef.current
      const len = history[index].length
      el?.setSelectionRange(len, len)
    })
  }

  const onKeyDown = (e: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    // IME 组合中的 Enter 是候选确认，不发送。
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault()
      void send()
      return
    }
    if (e.key === 'ArrowUp' && history.length > 0 && !disabled && (historyIndex !== null || value === '')) {
      // 仅在空输入或已在浏览历史时接管 ↑，避免打断多行编辑的光标移动。
      e.preventDefault()
      if (historyIndex === null) {
        recall(history.length - 1)
      } else if (historyIndex > 0) {
        recall(historyIndex - 1)
      }
      return
    }
    if (e.key === 'ArrowDown' && historyIndex !== null) {
      e.preventDefault()
      if (historyIndex < history.length - 1) {
        recall(historyIndex + 1)
      } else {
        setHistoryIndex(null)
        setValue('')
      }
    }
  }

  return (
    <div className="conversation-composer">
      {running && (
        <p className="mb-2 text-[11px] text-slate-500">{awaitingConfirmation ? '等待确认本次代码变更' : '任务运行中，输入暂时禁用…'}</p>
      )}
      {error && <p className="mb-2 text-[11px] text-rose-400">{error}</p>}
      <div className="composer-surface">
        <textarea
          ref={textareaRef}
          value={value}
          onChange={(e) => {
            setValue(e.target.value)
            // 手动编辑即退出历史浏览。
            if (historyIndex !== null) setHistoryIndex(null)
          }}
          onKeyDown={onKeyDown}
          disabled={disabled}
          aria-label="任务输入"
          placeholder={
            currentProject
              ? '描述你想完成的任务…'
              : '请先打开项目'
          }
          className="composer-input"
        />
        <div className="composer-bottom justify-end gap-2"><ModelPicker disabled={running || sending} /><div className="flex gap-2">
        {running && !awaitingConfirmation && currentTask && currentTask.id >= 0 && <button className="icon-button" aria-label="停止任务" title="停止任务" onClick={() => void ipc.cancelTask(currentTask.id).catch((e) => setError(toMessage(e)))}><Square size={14} /></button>}
        <button
          type="button"
          onClick={() => void send()}
          disabled={!canSend}
          aria-label="发送"
          title={
            currentProject === null
              ? '请先打开项目'
              : running
                ? '任务运行中'
                : '发送（Enter）'
          }
          className="send-button"
        >
          {sending ? <Loader2 size={17} className="animate-spin" /> : <ArrowUp size={19} />}
        </button>
        </div>
        </div>
      </div>
    </div>
  )
}
