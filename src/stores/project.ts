import { create } from 'zustand'

import { ipc } from '@/services/ipc'
import type { FileNode, Project } from '@/types'

/** 项目状态与操作：打开/关闭项目、文件树、最近项目列表。 */
export interface ProjectState {
  /** 当前打开的项目；未打开时为 null。 */
  currentProject: Project | null
  /** 当前项目的文件树。 */
  fileTree: FileNode | null
  /** 最近打开的项目（后端按最近打开时间倒序返回）。 */
  recentProjects: Project[]
  /** 项目级操作（打开/关闭）是否进行中。 */
  loading: boolean

  /** 打开项目：后端自动全量索引并启动增量监听，随后加载文件树并刷新最近列表。 */
  openProject: (path: string) => Promise<void>
  /** 关闭当前项目并清空相关状态；后端侧幂等。 */
  closeProject: () => Promise<void>
  /** 按项目 id 加载文件树。 */
  loadFileTree: (projectId: number) => Promise<void>
  /** 加载最近打开的项目列表。 */
  loadRecentProjects: () => Promise<void>
}

export const useProjectStore = create<ProjectState>()((set, get) => ({
  currentProject: null,
  fileTree: null,
  recentProjects: [],
  loading: false,

  openProject: async (path) => {
    set({ loading: true })
    try {
      const project = await ipc.openProject(path)
      set({ currentProject: project })
      await get().loadFileTree(project.id)
      await get().loadRecentProjects()
    } finally {
      set({ loading: false })
    }
  },

  closeProject: async () => {
    set({ loading: true })
    try {
      await ipc.closeProject()
      set({ currentProject: null, fileTree: null })
    } finally {
      set({ loading: false })
    }
  },

  loadFileTree: async (projectId) => {
    const fileTree = await ipc.getFileTree(projectId)
    set({ fileTree })
  },

  loadRecentProjects: async () => {
    const recentProjects = await ipc.listProjects()
    set({ recentProjects })
  },
}))
