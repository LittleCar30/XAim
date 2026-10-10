import { beforeEach, describe, expect, it, vi } from 'vitest'

import { applyTheme, loadTheme, THEME_KEY, useThemeStore } from './theme'

vi.mock('@/services/ipc', () => ({ ipc: { setPreference: vi.fn().mockResolvedValue(undefined) } }))

beforeEach(() => {
  localStorage.clear()
  useThemeStore.setState({ theme: 'light' })
  applyTheme('light')
})

describe('theme preferences', () => {
  it('defaults to light, including legacy dark preferences and invalid storage', () => {
    localStorage.setItem('xaim.settings-prefs', JSON.stringify({ ui: { theme: 'dark' } }))
    expect(loadTheme()).toBe('light')
    localStorage.setItem(THEME_KEY, 'invalid')
    expect(loadTheme()).toBe('light')
  })

  it('switches the document and persists the explicit choice across reloads', () => {
    useThemeStore.getState().setTheme('dark')
    expect(document.documentElement).toHaveClass('dark')
    expect(document.documentElement.dataset.theme).toBe('dark')
    expect(document.documentElement.style.colorScheme).toBe('dark')
    expect(loadTheme()).toBe('dark')
    useThemeStore.getState().setTheme('light')
    expect(document.documentElement).not.toHaveClass('dark')
    expect(loadTheme()).toBe('light')
  })

  it('still switches when local storage is unavailable', () => {
    const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('Storage blocked')
    })
    useThemeStore.getState().setTheme('dark')
    expect(document.documentElement).toHaveClass('dark')
    expect(useThemeStore.getState().theme).toBe('dark')
    write.mockRestore()
  })
})
