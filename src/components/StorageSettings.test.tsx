import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import StorageSettings from './StorageSettings'
import { ipc } from '@/services/ipc'
import type { StorageInfo } from '@/types'

vi.mock('@/services/ipc', () => ({ ipc: {
  getStorageInfo: vi.fn(), createDataBackup: vi.fn(), restoreDataBackup: vi.fn(),
  openDataFolder: vi.fn(), pickStorageFolder: vi.fn(), moveDataDirectory: vi.fn(), cancelDataMove: vi.fn(),
  cleanupWorkspaces: vi.fn(), setWorkspaceRetention: vi.fn(),
} }))
const mocked = vi.mocked(ipc, true)
const info: StorageInfo = { home: 'D:/XAim', database: 'D:/XAim/xaim.db', workspaces: 'D:/XAim/workspaces', backups: 'D:/XAim/backups',
  bytes: 2097152, workspaceCount: 3, retentionDays: 30, pendingHome: null, environmentOverride: false, notice: null }

beforeEach(() => { vi.resetAllMocks(); mocked.getStorageInfo.mockResolvedValue(info) })

it('shows the real data directory and reports the completed backup path', async () => {
  mocked.createDataBackup.mockResolvedValue('D:/XAim/backups/snapshot.db')
  render(<StorageSettings />)
  await screen.findByText('D:/XAim')
  expect(screen.getByText('2.0 MB · 3 个任务工作区')).toBeInTheDocument()
  fireEvent.click(screen.getByRole('button', { name: '立即备份' }))
  expect(await screen.findByText('备份已保存：D:/XAim/backups/snapshot.db')).toBeInTheDocument()
})

it('handles picker cancellation without scheduling a migration, then schedules a selected folder', async () => {
  mocked.pickStorageFolder.mockResolvedValueOnce(null).mockResolvedValueOnce('E:/XAim')
  mocked.moveDataDirectory.mockResolvedValue(undefined)
  render(<StorageSettings />)
  const button = await screen.findByRole('button', { name: '迁移数据目录…' })
  fireEvent.click(button)
  await waitFor(() => expect(button).toBeEnabled())
  expect(mocked.moveDataDirectory).not.toHaveBeenCalled()
  fireEvent.click(button)
  expect(await screen.findByText(/已安排迁移，重启 XAim 后生效/)).toBeInTheDocument()
  expect(mocked.moveDataDirectory).toHaveBeenCalledWith('E:/XAim')
})

it('keeps backup errors visible and allows retry', async () => {
  mocked.createDataBackup.mockRejectedValueOnce('disk full').mockResolvedValueOnce('D:/retry.db')
  render(<StorageSettings />)
  const button = await screen.findByRole('button', { name: '立即备份' })
  fireEvent.click(button)
  expect(await screen.findByRole('alert')).toHaveTextContent('disk full')
  fireEvent.click(button)
  expect(await screen.findByText('备份已保存：D:/retry.db')).toBeInTheDocument()
  expect(screen.queryByRole('alert')).not.toBeInTheDocument()
})

it('shows pending migration and disables changes controlled by XAIM_HOME', async () => {
  mocked.getStorageInfo.mockResolvedValue({ ...info, environmentOverride: true })
  const { unmount } = render(<StorageSettings />)
  expect(await screen.findByRole('button', { name: '迁移数据目录…' })).toBeDisabled()
  expect(screen.getByRole('button', { name: '恢复备份…' })).toBeDisabled()
  unmount()
  mocked.getStorageInfo.mockResolvedValue({ ...info, pendingHome: 'E:/XAim' })
  render(<StorageSettings />)
  expect(await screen.findByText(/下次启动迁移到：E:\/XAim/)).toBeInTheDocument()
  expect(screen.getByRole('button', { name: '取消迁移' })).toBeEnabled()
})
