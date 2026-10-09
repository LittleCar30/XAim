import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import TopBar from './TopBar'
import { ipc } from '@/services/ipc'
import { project, resetStores, tree } from '@/test/fixtures'
import { useProjectStore } from '@/stores/project'

vi.mock('@/services/ipc', () => ({ ipc: {
  pickProjectFolder: vi.fn(), openProject: vi.fn(), listProjects: vi.fn(), getFileTree: vi.fn(), getModelConfig: vi.fn(),
} }))
const mocked = vi.mocked(ipc, true)

describe('project folder selection', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    resetStores()
    mocked.listProjects.mockResolvedValue([])
    mocked.getFileTree.mockResolvedValue(tree)
  })

  it('opens the selected folder through the existing indexing workflow', async () => {
    const path = 'D:\\项目 文件夹\\demo'
    mocked.pickProjectFolder.mockResolvedValue(path)
    mocked.openProject.mockResolvedValue({ ...project, path })
    render(<TopBar />)
    fireEvent.click(screen.getByRole('button', { name: '打开项目' }))
    fireEvent.click(screen.getByRole('button', { name: '选择文件夹' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(mocked.pickProjectFolder).toHaveBeenCalledTimes(1)
    expect(mocked.openProject).toHaveBeenCalledWith(path)
    expect(mocked.getFileTree).toHaveBeenCalledWith(project.id)
    expect(useProjectStore.getState().currentProject?.path).toBe(path)
  })

  it('keeps the current project and typed path after cancelling the picker', async () => {
    useProjectStore.setState({ currentProject: project })
    mocked.pickProjectFolder.mockResolvedValue(null)
    render(<TopBar />)
    fireEvent.click(screen.getByRole('button', { name: '打开项目' }))
    fireEvent.change(screen.getByRole('textbox', { name: '项目路径' }), { target: { value: 'D:/typed' } })
    fireEvent.click(screen.getByRole('button', { name: '选择文件夹' }))
    await waitFor(() => expect(screen.getByRole('button', { name: '选择文件夹' })).toBeEnabled())
    expect(mocked.openProject).not.toHaveBeenCalled()
    expect(screen.getByRole('textbox', { name: '项目路径' })).toHaveValue('D:/typed')
    expect(useProjectStore.getState().currentProject).toEqual(project)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('shows picker errors and keeps manual path entry available', async () => {
    mocked.pickProjectFolder.mockRejectedValue(new Error('选择器不可用'))
    mocked.openProject.mockResolvedValue(project)
    render(<TopBar />)
    fireEvent.click(screen.getByRole('button', { name: '打开项目' }))
    fireEvent.click(screen.getByRole('button', { name: '选择文件夹' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('选择器不可用')
    fireEvent.change(screen.getByRole('textbox', { name: '项目路径' }), { target: { value: ' D:/repo ' } })
    fireEvent.click(screen.getByRole('button', { name: '打开' }))
    await waitFor(() => expect(mocked.openProject).toHaveBeenCalledWith('D:/repo'))
  })
})
