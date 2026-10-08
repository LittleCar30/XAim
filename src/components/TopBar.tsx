import { useEffect, useRef, useState } from 'react'

import { fullVersion } from '@/lib/version'
import { useConfigStore } from '@/stores/config'
import { useProjectStore } from '@/stores/project'

/** 顶部栏：品牌、项目名、索引状态、模型选择、打开项目与设置入口。 */
export interface TopBarProps {
  /** 打开设置页（由路由层提供）。 */
  onOpenSettings?: () => void
}

/** 顶栏当前展开的菜单。 */
type TopMenu = 'project' | null

/** IPC/store 抛出值的统一展示消息（Tauri 命令失败时 reject 的是 string）。 */
function toMessage(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  return String(e)
}

function FolderIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
    </svg>
  )
}

function GearIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  )
}

interface MenuShellProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

/** 打开项目弹层：路径输入 + 最近项目列表 + 关闭当前项目。 */
function ProjectMenu({ open, onOpenChange }: MenuShellProps) {
  const currentProject = useProjectStore((s) => s.currentProject)
  const recentProjects = useProjectStore((s) => s.recentProjects)
  const projectLoading = useProjectStore((s) => s.loading)
  const openProject = useProjectStore((s) => s.openProject)
  const closeProject = useProjectStore((s) => s.closeProject)
  const loadRecentProjects = useProjectStore((s) => s.loadRecentProjects)
  const [path, setPath] = useState('')
  const [error, setError] = useState<string | null>(null)

  // 打开时刷新最近项目；失败（非 Tauri 环境）静默降级。
  useEffect(() => {
    if (!open) return
    loadRecentProjects().catch(() => {})
  }, [open, loadRecentProjects])

  const handleOpen = async (target: string) => {
    const trimmed = target.trim()
    if (!trimmed) return
    setError(null)
    try {
      await openProject(trimmed)
      onOpenChange(false)
    } catch (e) {
      setError(toMessage(e))
    }
  }

  const handleClose = async () => {
    setError(null)
    try {
      await closeProject()
      onOpenChange(false)
    } catch (e) {
      setError(toMessage(e))
    }
  }

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => onOpenChange(!open)}
        aria-haspopup="dialog"
        aria-expanded={open}
        className="flex items-center gap-1.5 rounded-md px-2 py-1.5 text-xs text-slate-300 hover:bg-slate-800 hover:text-slate-100"
      >
        <FolderIcon />
        打开项目
      </button>

      {open && (
        <div
          role="dialog"
          aria-label="打开项目"
          className="absolute right-0 top-full z-50 mt-1 w-80 rounded-lg border border-slate-700 bg-slate-900 p-3 shadow-xl"
        >
          <form
            onSubmit={(e) => {
              e.preventDefault()
              handleOpen(path)
            }}
            className="flex gap-2"
          >
            <input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="输入项目路径，如 D:\code\demo"
              aria-label="项目路径"
              className="min-w-0 flex-1 rounded-md bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100 outline-none ring-1 ring-slate-700 placeholder:text-slate-600 focus:ring-sky-500"
            />
            <button
              type="submit"
              disabled={!path.trim() || projectLoading}
              className="shrink-0 rounded-md bg-sky-500 px-3 py-1.5 text-xs font-medium text-slate-950 hover:bg-sky-400 disabled:cursor-not-allowed disabled:opacity-50"
            >
              打开
            </button>
          </form>

          {error && <p className="mt-2 break-all text-xs text-rose-400">{error}</p>}

          {recentProjects.length > 0 && (
            <div className="mt-3">
              <p className="text-xs text-slate-500">最近项目</p>
              <ul className="mt-1 max-h-[12rem] overflow-auto">
                {recentProjects.map((p) => (
                  <li key={p.id}>
                    <button
                      type="button"
                      disabled={projectLoading}
                      onClick={() => handleOpen(p.path)}
                      title={p.path}
                      className="flex w-full flex-col rounded px-2 py-1.5 text-left hover:bg-slate-800 disabled:opacity-50"
                    >
                      <span className="truncate text-xs text-slate-200">{p.name}</span>
                      <span className="truncate text-[11px] text-slate-500">{p.path}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {currentProject && (
            <button
              type="button"
              onClick={handleClose}
              disabled={projectLoading}
              className="mt-3 w-full rounded-md border border-slate-700 px-3 py-1.5 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-50"
            >
              关闭当前项目
            </button>
          )}
        </div>
      )}
    </div>
  )
}

export default function TopBar({ onOpenSettings }: TopBarProps) {
  const [openMenu, setOpenMenu] = useState<TopMenu>(null)
  const rootRef = useRef<HTMLElement | null>(null)

  const projectName = useProjectStore((s) => s.currentProject?.name)
  const projectLoading = useProjectStore((s) => s.loading)
  const hasProject = useProjectStore((s) => s.currentProject !== null)
  const loadModelConfig = useConfigStore((s) => s.loadModelConfig)

  // 挂载时补拉一次模型配置（供下拉展示）；失败（非 Tauri 环境）保持未配置态。
  const triedConfigRef = useRef(false)
  useEffect(() => {
    if (triedConfigRef.current) return
    triedConfigRef.current = true
    loadModelConfig().catch(() => {})
  }, [loadModelConfig])

  // 点击菜单外或按 Esc 关闭。
  useEffect(() => {
    if (!openMenu) return
    const onPointerDown = (e: PointerEvent) => {
      if (rootRef.current && e.target instanceof Node && !rootRef.current.contains(e.target)) {
        setOpenMenu(null)
      }
    }
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpenMenu(null)
    }
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('pointerdown', onPointerDown)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [openMenu])

  // 索引状态：打开/关闭项目进行中 → 索引中；已打开 → 就绪；否则未索引。
  const indexStatus = projectLoading
    ? { dot: 'bg-amber-400 animate-pulse', label: '索引中', title: '正在打开项目并建立索引…' }
    : hasProject
      ? { dot: 'bg-emerald-400', label: '已索引', title: '项目索引就绪（增量监听中）' }
      : { dot: 'bg-slate-600', label: '未索引', title: '尚未打开项目' }

  return (
    <header
      ref={rootRef}
      className="relative flex h-12 shrink-0 items-center gap-3 border-b border-slate-800 bg-slate-900 px-4"
    >
      <img src="/xaim.svg" width="26" height="26" alt="" className="shrink-0" />
      <span className="shrink-0 text-sm font-semibold text-slate-100" title={fullVersion()}>XAim</span>

      {projectName && (
        <div className="flex min-w-0 items-center gap-3">
          <span className="h-4 w-px shrink-0 bg-slate-700" aria-hidden="true" />
          <span className="truncate text-sm text-slate-300" title={projectName}>
            {projectName}
          </span>
        </div>
      )}

      <span
        className="hidden shrink-0 items-center gap-1.5 text-xs text-slate-500 sm:flex"
        title={indexStatus.title}
      >
        <span className={`h-2 w-2 rounded-full ${indexStatus.dot}`} aria-hidden="true" />
        {indexStatus.label}
      </span>

      <div className="ml-auto flex shrink-0 items-center gap-1">
        <ProjectMenu
          open={openMenu === 'project'}
          onOpenChange={(o) => setOpenMenu(o ? 'project' : null)}
        />
        {onOpenSettings && (
          <button
            type="button"
            onClick={onOpenSettings}
            aria-label="打开设置"
            title="设置"
            className="rounded-md p-1.5 text-slate-400 hover:bg-slate-800 hover:text-slate-100"
          >
            <GearIcon />
          </button>
        )}
      </div>
    </header>
  )
}
