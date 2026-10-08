import { beforeEach, describe, expect, it, vi } from 'vitest'

import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import type { FileNode, Project } from '@/types'

vi.mock('@/services/ipc', () => ({
  ipc: {
    openProject: vi.fn(),
    closeProject: vi.fn(),
    getFileTree: vi.fn(),
    listProjects: vi.fn(),
  },
}))

const mockedIpc = vi.mocked(ipc, true)

const project: Project = {
  id: 1,
  path: 'D:/repo',
  name: 'repo',
  createdAt: 100,
  lastOpenedAt: 200,
}

const tree: FileNode = {
  name: 'repo',
  path: '',
  isDir: true,
  size: null,
  children: [{ name: 'main.rs', path: 'src/main.rs', isDir: false, size: 12, children: [] }],
}

describe('useProjectStore', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useProjectStore.setState({
      currentProject: null,
      fileTree: null,
      recentProjects: [],
      loading: false,
    })
  })

  it('has empty initial state', () => {
    const state = useProjectStore.getState()
    expect(state.currentProject).toBeNull()
    expect(state.fileTree).toBeNull()
    expect(state.recentProjects).toEqual([])
    expect(state.loading).toBe(false)
  })

  describe('openProject', () => {
    it('sets current project, loads file tree and refreshes recents', async () => {
      mockedIpc.openProject.mockResolvedValue(project)
      mockedIpc.getFileTree.mockResolvedValue(tree)
      mockedIpc.listProjects.mockResolvedValue([project])

      await useProjectStore.getState().openProject('D:/repo')

      expect(mockedIpc.openProject).toHaveBeenCalledWith('D:/repo')
      expect(mockedIpc.getFileTree).toHaveBeenCalledWith(1)
      expect(mockedIpc.listProjects).toHaveBeenCalledTimes(1)
      expect(useProjectStore.getState().currentProject).toEqual(project)
      expect(useProjectStore.getState().fileTree).toEqual(tree)
      expect(useProjectStore.getState().recentProjects).toEqual([project])
      expect(useProjectStore.getState().loading).toBe(false)
    })

    it('resets loading and keeps state untouched on failure', async () => {
      mockedIpc.openProject.mockRejectedValue(new Error('not a project'))

      await expect(
        useProjectStore.getState().openProject('D:/repo'),
      ).rejects.toThrow('not a project')

      const state = useProjectStore.getState()
      expect(state.loading).toBe(false)
      expect(state.currentProject).toBeNull()
    })

    it('exposes loading while opening', async () => {
      let resolveOpen!: (p: Project) => void
      mockedIpc.openProject.mockImplementation(
        () => new Promise<Project>((resolve) => (resolveOpen = resolve)),
      )

      const pending = useProjectStore.getState().openProject('D:/repo')
      expect(useProjectStore.getState().loading).toBe(true)

      resolveOpen(project)
      mockedIpc.getFileTree.mockResolvedValue(tree)
      mockedIpc.listProjects.mockResolvedValue([])
      await pending
      expect(useProjectStore.getState().loading).toBe(false)
    })
  })

  describe('closeProject', () => {
    it('clears current project and file tree', async () => {
      useProjectStore.setState({ currentProject: project, fileTree: tree })
      mockedIpc.closeProject.mockResolvedValue(undefined)

      await useProjectStore.getState().closeProject()

      expect(mockedIpc.closeProject).toHaveBeenCalledTimes(1)
      const state = useProjectStore.getState()
      expect(state.currentProject).toBeNull()
      expect(state.fileTree).toBeNull()
      expect(state.loading).toBe(false)
    })

    it('resets loading on failure', async () => {
      mockedIpc.closeProject.mockRejectedValue(new Error('backend gone'))

      await expect(useProjectStore.getState().closeProject()).rejects.toThrow('backend gone')
      expect(useProjectStore.getState().loading).toBe(false)
    })
  })

  describe('loadFileTree', () => {
    it('stores the tree returned by the backend', async () => {
      mockedIpc.getFileTree.mockResolvedValue(tree)

      await useProjectStore.getState().loadFileTree(1)

      expect(mockedIpc.getFileTree).toHaveBeenCalledWith(1)
      expect(useProjectStore.getState().fileTree).toEqual(tree)
    })
  })

  describe('loadRecentProjects', () => {
    it('stores the recent project list', async () => {
      const recents = [project, { ...project, id: 2, name: 'other' }]
      mockedIpc.listProjects.mockResolvedValue(recents)

      await useProjectStore.getState().loadRecentProjects()

      expect(useProjectStore.getState().recentProjects).toEqual(recents)
    })
  })
})
