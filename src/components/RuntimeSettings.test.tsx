import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import RuntimeSettings from './RuntimeSettings'
import { ipc } from '@/services/ipc'
import { useProjectStore } from '@/stores/project'
import { project } from '@/test/fixtures'

vi.mock('@/services/ipc', () => ({ ipc: { getRuntimeSettings: vi.fn(), setRuntimeSettings: vi.fn() } }))
const mocked = vi.mocked(ipc, true)
beforeEach(() => {
  vi.resetAllMocks()
  useProjectStore.setState({ currentProject: project })
  mocked.getRuntimeSettings.mockResolvedValue({ defaults: {}, shared: {}, local: {}, effective: { taskTimeoutSec: 1800, maxTurns: 32 } })
  mocked.setRuntimeSettings.mockResolvedValue(undefined)
})

it('saves validated execution limits to the selected project and can restore inheritance', async () => {
  render(<RuntimeSettings />)
  const scope = screen.getByLabelText('任务设置范围')
  await waitFor(() => expect(scope).toBeEnabled())
  fireEvent.change(scope, { target: { value: 'project' } })
  await waitFor(() => expect(screen.getByLabelText('任务总时限（秒）')).toBeEnabled())
  fireEvent.change(screen.getByLabelText('任务总时限（秒）'), { target: { value: '600' } })
  fireEvent.change(screen.getByLabelText('工具循环最大轮数'), { target: { value: '12' } })
  fireEvent.click(screen.getByRole('button', { name: '保存任务设置' }))
  await screen.findByText('任务设置已保存')
  expect(mocked.setRuntimeSettings).toHaveBeenLastCalledWith(project.id, { taskTimeoutSec: 600, maxTurns: 12 })
  fireEvent.click(screen.getByRole('button', { name: '恢复继承设置' }))
  await waitFor(() => expect(mocked.setRuntimeSettings).toHaveBeenLastCalledWith(project.id, {}))
})

it('rejects invalid budgets before writing them', async () => {
  render(<RuntimeSettings />)
  const input = screen.getByLabelText('工具循环最大轮数')
  await waitFor(() => expect(input).toBeEnabled())
  fireEvent.change(input, { target: { value: '0' } })
  fireEvent.click(screen.getByRole('button', { name: '保存任务设置' }))
  expect(screen.getByRole('alert')).toHaveTextContent('1–256')
  expect(mocked.setRuntimeSettings).not.toHaveBeenCalled()
})
