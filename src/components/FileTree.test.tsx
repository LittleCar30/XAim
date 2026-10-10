import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import FileTree from './FileTree'
import { useProjectStore } from '@/stores/project'
import { useUiStore } from '@/stores/ui'
import { useTaskStore } from '@/stores/task'
import { patch, resetStores, task } from '@/test/fixtures'

const tree = {
  name: 'repo', path: '.', isDir: true, size: null,
  children: [
    { name: 'src', path: 'src', isDir: true, size: null, children: [
      { name: 'main.ts', path: 'src/main.ts', isDir: false, size: 2048, children: [] },
      { name: 'readme.md', path: 'src/readme.md', isDir: false, size: 10, children: [] },
    ] },
    { name: 'package.json', path: 'package.json', isDir: false, size: 100, children: [] },
  ],
}

describe('FileTree', () => {
  beforeEach(() => {
    resetStores()
    useProjectStore.setState({ fileTree: null, currentProject: null, recentProjects: [], loading: false })
    useUiStore.setState({ sidebarOpen: true, rightPanelOpen: true, previewPath: null })
  })

  it('renders closed folders, expands them, selects files, and supports keyboard navigation', async () => {
    useProjectStore.setState({ fileTree: tree })
    render(<FileTree />)
    expect(screen.getByRole('tree')).toBeInTheDocument()
    expect(screen.getByText('src')).toBeInTheDocument()
    expect(screen.queryByText('main.ts')).not.toBeInTheDocument()
    fireEvent.click(screen.getByText('src'))
    expect(screen.getByText('main.ts')).toBeInTheDocument()
    fireEvent.click(screen.getByText('main.ts'))
    expect(useUiStore.getState().previewPath).toBe('src/main.ts')
    const treeRole = screen.getByRole('tree')
    fireEvent.keyDown(treeRole, { key: 'Home' })
    fireEvent.keyDown(treeRole, { key: 'ArrowDown' })
    fireEvent.keyDown(treeRole, { key: 'ArrowRight' })
    expect(treeRole).toHaveAttribute('aria-activedescendant')
    await waitFor(() => expect(screen.getByText('main.ts')).toBeInTheDocument())
  })

  it('searches descendants and highlights matched file names', () => {
    useProjectStore.setState({ fileTree: tree })
    render(<FileTree />)
    fireEvent.change(screen.getByRole('textbox', { name: '搜索文件' }), { target: { value: 'MAIN' } })
    const file = screen.getByRole('treeitem', { name: /main\s*\.ts/ })
    expect(file).toBeInTheDocument()
    expect(screen.queryByText('package.json')).not.toBeInTheDocument()
    expect(file.querySelector('mark')).toHaveTextContent('main')
    fireEvent.click(screen.getByRole('button', { name: '清除搜索' }))
    expect(screen.getByPlaceholderText('搜索文件…')).toHaveValue('')
  })

  it('shows a newly generated file before it is applied', () => {
    useProjectStore.setState({ fileTree: tree })
    useTaskStore.setState({ currentTask: { ...task, status: 'waitingConfirm' }, patches: [{ ...patch, filePath: 'new.ts', applied: false }] })
    render(<FileTree />)
    expect(screen.getByText('new.ts')).toBeInTheDocument()
    expect(screen.getByText('待应用')).toBeInTheDocument()
    fireEvent.click(screen.getByText('new.ts'))
    expect(useUiStore.getState().previewPath).toBe('new.ts')
  })
})

