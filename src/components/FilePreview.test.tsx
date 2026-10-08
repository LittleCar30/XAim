import { render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import FilePreview from './FilePreview'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'

vi.mock('@/services/ipc', () => ({ ipc: { readFile: vi.fn() } }))
const mockedIpc = vi.mocked(ipc, true)

describe('FilePreview', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useProjectStore.setState({ currentProject: { id: 7, path: 'repo', name: 'repo', createdAt: 0, lastOpenedAt: 0 }, fileTree: null, recentProjects: [], loading: false })
  })

  it('loads file content with line range and renders line numbers', async () => {
    mockedIpc.readFile.mockResolvedValue('first\nsecond')
    render(<FilePreview path="src/main.ts" onClose={vi.fn()} />)
    expect(screen.getByText('加载文件…')).toBeInTheDocument()
    await waitFor(() => expect(screen.getByText('first')).toBeInTheDocument())
    expect(mockedIpc.readFile).toHaveBeenCalledWith(7, 'src/main.ts', 1, 2000)
    expect(screen.getByText('1')).toBeInTheDocument()
    expect(screen.getByText('2')).toBeInTheDocument()
  })

  it('shows backend errors and returns to context', async () => {
    const onClose = vi.fn()
    mockedIpc.readFile.mockRejectedValue('permission denied')
    render(<FilePreview path="secret.ts" onClose={onClose} />)
    await waitFor(() => expect(screen.getByText('permission denied')).toBeInTheDocument())
    screen.getByRole('button', { name: '返回上下文' }).click()
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('reports missing project without calling IPC', () => {
    useProjectStore.setState({ currentProject: null })
    render(<FilePreview path="main.ts" onClose={vi.fn()} />)
    expect(screen.getByText('未打开项目，无法读取文件。')).toBeInTheDocument()
    expect(mockedIpc.readFile).not.toHaveBeenCalled()
  })
})

