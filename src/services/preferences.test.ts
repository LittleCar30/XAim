import { beforeEach, expect, it, vi } from 'vitest'
import { hydratePreferences, savePreference } from './preferences'
import { ipc } from './ipc'

vi.mock('./ipc', () => ({ ipc: { getPreferences: vi.fn(), setPreference: vi.fn() } }))
const mocked = vi.mocked(ipc, true)
beforeEach(() => { vi.resetAllMocks(); localStorage.clear(); mocked.setPreference.mockResolvedValue(undefined) })

it('restores backed-up preferences over stale WebView values and migrates missing legacy preferences', async () => {
  localStorage.setItem('xaim.theme', 'light')
  localStorage.setItem('xaim.prompt-history', '["legacy"]')
  mocked.getPreferences.mockResolvedValue({ 'xaim.theme': 'dark' })
  await hydratePreferences()
  expect(localStorage.getItem('xaim.theme')).toBe('dark')
  expect(mocked.setPreference).toHaveBeenCalledTimes(1)
  expect(mocked.setPreference).toHaveBeenCalledWith('xaim.prompt-history', '["legacy"]')
})

it('persists preferences even when the WebView cache is unavailable', async () => {
  const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('cache blocked') })
  try {
    await savePreference('xaim.theme', 'dark')
    expect(mocked.setPreference).toHaveBeenCalledWith('xaim.theme', 'dark')
  } finally { write.mockRestore() }
})

it('surfaces a database write failure while retaining the usable local value', async () => {
  mocked.setPreference.mockRejectedValueOnce('disk full')
  await expect(savePreference('xaim.theme', 'dark')).rejects.toBe('disk full')
  expect(localStorage.getItem('xaim.theme')).toBe('dark')
})
