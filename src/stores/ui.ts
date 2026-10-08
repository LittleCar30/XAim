import { create } from 'zustand'

/** 界面状态：面板折叠等纯前端视图状态。 */
export interface UiStore {
  detailTab: 'context' | 'tools' | 'verify'
  /** 左侧文件树是否展开。 */
  sidebarOpen: boolean
  /** 右侧面板（上下文查看器）是否展开。 */
  rightPanelOpen: boolean
  /** 当前预览的文件路径（来自文件树点选）；null 表示右栏显示上下文查看器。 */
  previewPath: string | null
  /** 切换左侧文件树展开/折叠。 */
  toggleSidebar: () => void
  /** 切换右侧面板展开/折叠。 */
  toggleRightPanel: () => void
  /** 设置/清除预览文件（传 null 返回上下文）。 */
  setPreviewPath: (path: string | null) => void
}

export const useUiStore = create<UiStore>()((set) => ({
  detailTab: 'context',
  sidebarOpen: true,
  rightPanelOpen: false,
  previewPath: null,
  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  toggleRightPanel: () => set((s) => ({ rightPanelOpen: !s.rightPanelOpen })),
  setPreviewPath: (path) => set(path ? { previewPath: path, rightPanelOpen: true } : { previewPath: path }),
}))
