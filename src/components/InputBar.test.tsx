import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import InputBar from './InputBar'
import TaskPanel from './TaskPanel'
import { useConfigStore } from '@/stores/config'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { useTaskStore } from '@/stores/task'
import { patch, project, resetStores, task } from '@/test/fixtures'

vi.mock('@/services/ipc', () => ({ ipc: { runTask: vi.fn(), getTask: vi.fn(), listPatches: vi.fn(), setPreference: vi.fn() } }))
const mocked = vi.mocked(ipc, true)

describe('InputBar', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    resetStores()
    localStorage.clear()
  })

  it('disables input without a project and while task is running', () => {
    render(<InputBar />)
    expect(screen.getByRole('textbox', { name: '任务输入' })).toBeDisabled()
    act(() => {
      useProjectStore.setState({ currentProject: project })
      useTaskStore.setState({ currentTask: { ...task, status: 'generating' } })
    })
    expect(screen.getByText('任务运行中，输入暂时禁用…')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '发送' })).toBeDisabled()
  })

  it('sends trimmed prompt with Enter, loads task and persists history', async () => {
    useProjectStore.setState({ currentProject: project })
    mocked.runTask.mockResolvedValue(task.id)
    mocked.getTask.mockResolvedValue(task)
    mocked.listPatches.mockResolvedValue([patch])
    useTaskStore.setState({ patches: [{ ...patch, id: 0 }], verifies: [{ kind: 'test', passed: false, output: 'old task', durationMs: 0 }] })
    render(<InputBar />)
    const input = screen.getByRole('textbox', { name: '任务输入' })
    fireEvent.change(input, { target: { value: '  fix login  ' } })
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
    fireEvent.keyDown(input, { key: 'Enter', isComposing: true })
    expect(mocked.runTask).not.toHaveBeenCalled()
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(input).toHaveValue(''))
    expect(mocked.runTask).toHaveBeenCalledWith(7, 'fix login', false)
    expect(mocked.getTask).toHaveBeenCalledWith(42)
    expect(useTaskStore.getState().currentTask).toEqual(task)
    expect(mocked.listPatches).toHaveBeenCalledWith(42)
    expect(useTaskStore.getState().patches).toEqual([patch])
    expect(useTaskStore.getState().verifies).toEqual([])
    expect(JSON.parse(localStorage.getItem('xaim.prompt-history')!)).toEqual(['fix login'])
  })

  it('recalls history up/down and exits history mode on editing', () => {
    useProjectStore.setState({ currentProject: project })
    localStorage.setItem('xaim.prompt-history', JSON.stringify(['first', 'second', 3]))
    render(<InputBar />)
    const input = screen.getByRole('textbox')
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(input).toHaveValue('second')
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(input).toHaveValue('first')
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(input).toHaveValue('second')
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    expect(input).toHaveValue('')
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    fireEvent.change(input, { target: { value: 'edited' } })
    fireEvent.keyDown(input, { key: 'ArrowUp' })
    expect(input).toHaveValue('edited')
  })

  it('retains prompt and re-enables input after IPC failure', async () => {
    useProjectStore.setState({ currentProject: project })
    localStorage.setItem('xaim.prompt-history', '{broken')
    mocked.runTask.mockRejectedValueOnce('offline')
    render(<InputBar />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'retry me' } })
    fireEvent.click(screen.getByRole('button', { name: '发送' }))
    expect(await screen.findByText('offline')).toBeInTheDocument()
    expect(screen.getByRole('textbox')).toHaveValue('retry me')
    expect(screen.getByRole('button', { name: '发送' })).toBeEnabled()
  })

  it('consumes verification approval for only one task', async () => {
    useProjectStore.setState({ currentProject: project })
    mocked.runTask.mockRejectedValue('offline')
    render(<InputBar />)
    const checkbox = screen.getByRole('checkbox', { name: '运行项目检查' })
    expect(checkbox).not.toBeChecked()
    fireEvent.click(checkbox)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'check code' } })
    fireEvent.click(screen.getByRole('button', { name: '发送' }))
    await waitFor(() => expect(screen.getByRole('button', { name: '发送' })).toBeEnabled())
    expect(mocked.runTask).toHaveBeenLastCalledWith(project.id, 'check code', true)
    expect(checkbox).not.toBeChecked()
    fireEvent.click(screen.getByRole('button', { name: '发送' }))
    await waitFor(() => expect(mocked.runTask).toHaveBeenCalledTimes(2))
    expect(mocked.runTask).toHaveBeenLastCalledWith(project.id, 'check code', false)
  })

  it('shows a backend failure only once and preserves cancellation status', async () => {
    useProjectStore.setState({ currentProject: project })
    mocked.runTask.mockImplementation(async () => {
      useTaskStore.getState().handleEvent({ type: 'error', taskId: task.id, message: 'invalid patch' })
      useTaskStore.getState().handleEvent({ type: 'done', taskId: task.id, status: 'failed' })
      throw 'invalid patch'
    })
    render(<><TaskPanel /><InputBar /></>)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'create page' } })
    fireEvent.click(screen.getByRole('button', { name: '发送' }))
    await waitFor(() => expect(screen.getByRole('button', { name: '发送' })).toBeEnabled())
    expect(screen.getAllByText('invalid patch')).toHaveLength(1)
    mocked.runTask.mockImplementation(async () => {
      useTaskStore.getState().handleEvent({ type: 'done', taskId: 43, status: 'cancelled' })
      throw 'cancelled'
    })
    fireEvent.click(screen.getByRole('button', { name: '发送' }))
    await waitFor(() => expect(screen.getByRole('button', { name: '发送' })).toBeEnabled())
    expect(useTaskStore.getState().currentTask?.status).toBe('cancelled')
  })

  it('blocks Enter while model settings are saving', () => {
    useProjectStore.setState({ currentProject: project })
    render(<InputBar />)
    const input = screen.getByRole('textbox')
    fireEvent.change(input, { target: { value: 'create page' } })
    act(() => useConfigStore.setState({ loading: true }))
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(mocked.runTask).not.toHaveBeenCalled()
  })
})
