import { useCallback, useEffect, useRef, useState } from 'react'
import { Files, MessageSquare, Plus, RefreshCw } from 'lucide-react'
import FileTree from '@/components/FileTree'
import { formatTaskTime, TASK_STATUS_LABELS, toMessage } from '@/lib/task-format'
import { useConversationStore } from '@/stores/conversation'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { ipc } from '@/services/ipc'

export default function ConversationSidebar({ onNavigate }: { onNavigate?: () => void }) {
  const [tab, setTab] = useState<'chats' | 'files'>('chats')
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const refreshVersion = useRef(0)
  const project = useProjectStore((s) => s.currentProject)
  const history = useTaskStore((s) => s.history)
  const currentTask = useTaskStore((s) => s.currentTask)
  const selectedId = useConversationStore((s) => s.selectedId)
  const select = useConversationStore((s) => s.select)
  const busy = currentTask !== null && !['idle', 'done', 'applied', 'failed', 'cancelled', 'rolledBack'].includes(currentTask.status)
  const refresh = useCallback(async () => {
    if (!project) return
    const version = ++refreshVersion.current
    setLoading(true)
    try {
      const tasks = await ipc.listTasks(project.id)
      if (version !== refreshVersion.current || useProjectStore.getState().currentProject?.id !== project.id) return
      useTaskStore.getState().setHistory(tasks)
      setError(null)
    } catch (error) { if (version === refreshVersion.current) setError(toMessage(error)) }
    finally { if (version === refreshVersion.current) setLoading(false) }
  }, [project])
  useEffect(() => { void refresh() }, [refresh, currentTask?.id, currentTask?.status])
  useEffect(() => () => { refreshVersion.current++ }, [project?.id])
  const conversations = currentTask && currentTask.id >= 0
    ? [currentTask, ...history.filter((task) => task.id !== currentTask.id)] : history
  const activeId = selectedId ?? currentTask?.id
  const newConversation = () => {
    useConversationStore.getState().archiveCurrent()
    const history = useTaskStore.getState().history
    useTaskStore.getState().reset()
    useTaskStore.setState({ history })
    void select(null)
    setTab('chats')
    onNavigate?.()
  }
  return (
    <aside className="conversation-sidebar" aria-label="项目导航">
      <div className="sidebar-actions">
        <button className="new-chat" disabled={busy || !project} onClick={newConversation}><Plus size={17} />新对话</button>
        <div className="sidebar-tabs" role="tablist" aria-label="项目视图">
          <button role="tab" aria-selected={tab === 'chats'} onClick={() => setTab('chats')}><MessageSquare size={15} />对话</button>
          <button role="tab" aria-selected={tab === 'files'} onClick={() => setTab('files')}><Files size={15} />文件</button>
        </div>
      </div>
      {tab === 'files' ? <FileTree /> : <div className="chat-history">
        <div className="sidebar-heading"><span>最近对话</span><button className="icon-button" title="刷新历史对话" aria-label="刷新历史对话" disabled={loading || !project} onClick={() => void refresh()}><RefreshCw size={14} className={loading ? 'animate-spin' : ''} /></button></div>
        {error && <p className="sidebar-error">{error}</p>}
        {!project && <p className="sidebar-empty">未打开项目</p>}
        {project && conversations.length === 0 && <p className="sidebar-empty">暂无对话</p>}
        <nav aria-label="历史对话">{conversations.map((task) => <button key={task.id} className={`history-item ${activeId === task.id ? 'selected' : ''}`} aria-current={activeId === task.id ? 'page' : undefined} onClick={() => { void select(task.id); onNavigate?.() }} title={task.prompt}>
          <span className="history-prompt">{task.prompt}</span><span className="history-meta"><span>{TASK_STATUS_LABELS[task.status]}</span><span>{formatTaskTime(task.createdAt)}</span></span>
        </button>)}</nav>
      </div>}
      <div className="sidebar-project"><span className="project-dot" /><span>{project?.name ?? 'XAim'}</span></div>
    </aside>
  )
}
