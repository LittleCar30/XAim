import { ArrowLeft, Menu, PanelRight, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import ConversationSidebar from '@/components/ConversationSidebar'
import ContextViewer from '@/components/ContextViewer'
import FilePreview from '@/components/FilePreview'
import InputBar from '@/components/InputBar'
import StatusBar from '@/components/StatusBar'
import TaskPanel from '@/components/TaskPanel'
import ToolLog from '@/components/ToolLog'
import TopBar from '@/components/TopBar'
import VerifyPanel from '@/components/VerifyPanel'
import { useConversationStore } from '@/stores/conversation'
import { useTaskStore } from '@/stores/task'
import { useUiStore } from '@/stores/ui'
import { useProjectStore } from '@/stores/project'

export interface WorkspaceProps { onOpenSettings?: () => void }

export default function Workspace({ onOpenSettings }: WorkspaceProps) {
  const [mobileNavigation, setMobileNavigation] = useState(false)
  const sidebarOpen = useUiStore((s) => s.sidebarOpen)
  const rightPanelOpen = useUiStore((s) => s.rightPanelOpen)
  const toggleRightPanel = useUiStore((s) => s.toggleRightPanel)
  const toggleSidebar = useUiStore((s) => s.toggleSidebar)
  const previewPath = useUiStore((s) => s.previewPath)
  const setPreviewPath = useUiStore((s) => s.setPreviewPath)
  const detailTab = useUiStore((s) => s.detailTab)
  const selectedId = useConversationStore((s) => s.selectedId)
  const selected = useConversationStore((s) => s.record)
  const verifies = useTaskStore((s) => s.verifies)
  const projectId = useProjectStore((s) => s.currentProject?.id ?? null)
  useEffect(() => { useConversationStore.getState().syncProject(projectId) }, [projectId])
  return (
    <div className="conversation-workspace">
      <TopBar onOpenSettings={onOpenSettings} />
      <div className={`workspace-body ${mobileNavigation ? 'mobile-navigation-open' : ''}`}>
        {(sidebarOpen || mobileNavigation) && <ConversationSidebar onNavigate={() => setMobileNavigation(false)} />}
        {mobileNavigation && <button className="navigation-backdrop" aria-label="关闭侧栏" onClick={() => setMobileNavigation(false)} />}
        <main className="conversation-main">
          <header className="conversation-heading">
            <button className="icon-button" onClick={() => { if (window.innerWidth < 768) setMobileNavigation((open) => !open); else toggleSidebar() }} aria-label="切换侧栏" title="切换侧栏"><Menu size={18} /></button>
            <span>{selectedId === null ? '对话' : '历史对话'}</span>
            {selectedId !== null && <button className="return-current" onClick={() => void useConversationStore.getState().select(null)}><ArrowLeft size={14} />返回当前对话</button>}
            <button className={`icon-button detail-toggle ${rightPanelOpen ? 'active' : ''}`} onClick={toggleRightPanel} aria-label="任务详情" title="任务详情" aria-expanded={rightPanelOpen}><PanelRight size={18} /></button>
          </header>
          <TaskPanel />
          <InputBar />
        </main>
        {rightPanelOpen && <aside className="task-inspector" aria-label="任务详情面板">
          <header className="inspector-heading">{previewPath && <button className="icon-button" onClick={() => setPreviewPath(null)} aria-label="返回任务详情" title="返回任务详情"><ArrowLeft size={17} /></button>}<span>{previewPath ? previewPath.split(/[\\/]/).pop() : '任务详情'}</span><button className="icon-button" onClick={toggleRightPanel} aria-label="关闭任务详情" title="关闭"><X size={17} /></button></header>
          {previewPath ? <FilePreview path={previewPath} onClose={() => setPreviewPath(null)} /> : <>
            <div className="inspector-tabs" role="tablist" aria-label="详情视图">{(['context', 'tools', 'verify'] as const).map((tab) => <button key={tab} role="tab" aria-selected={detailTab === tab} onClick={() => useUiStore.setState({ detailTab: tab })}>{{ context: '上下文', tools: '工具日志', verify: '验证' }[tab]}</button>)}</div>
            {detailTab === 'verify' ? <VerifyPanel verifies={selectedId === null ? verifies : selected?.verifies ?? []} /> : detailTab === 'context' ? <ContextViewer /> : selectedId !== null ? <p className="inspector-empty">此历史任务未保存工具日志</p> : <ToolLog />}
          </>}
        </aside>}
      </div>
      <StatusBar task={selectedId === null ? undefined : selected?.task ?? null} />
    </div>
  )
}
