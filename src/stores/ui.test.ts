import { beforeEach, expect, it } from 'vitest'
import { useUiStore } from './ui'

beforeEach(() => useUiStore.setState({ sidebarOpen: true, rightPanelOpen: true, previewPath: null }))

it('toggles panels independently and clears selected preview', () => {
  useUiStore.getState().toggleSidebar()
  expect(useUiStore.getState().sidebarOpen).toBe(false)
  expect(useUiStore.getState().rightPanelOpen).toBe(true)
  useUiStore.getState().toggleRightPanel()
  expect(useUiStore.getState().rightPanelOpen).toBe(false)
  useUiStore.getState().setPreviewPath('src/main.ts')
  expect(useUiStore.getState().previewPath).toBe('src/main.ts')
  useUiStore.getState().setPreviewPath(null)
  expect(useUiStore.getState().previewPath).toBeNull()
  useUiStore.getState().toggleSidebar()
  expect(useUiStore.getState().sidebarOpen).toBe(true)
})
