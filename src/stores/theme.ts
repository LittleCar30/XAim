import { create } from 'zustand'
import { savePreference } from '@/services/preferences'

export type Theme = 'light' | 'dark'
export const THEME_KEY = 'xaim.theme'

export function loadTheme(): Theme {
  try {
    return localStorage.getItem(THEME_KEY) === 'dark' ? 'dark' : 'light'
  } catch {
    return 'light'
  }
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme
  document.documentElement.classList.toggle('dark', theme === 'dark')
  document.documentElement.style.colorScheme = theme
}

export const useThemeStore = create<{
  theme: Theme
  setTheme: (theme: Theme) => void
}>()((set) => ({
  theme: loadTheme(),
  setTheme: (theme) => {
    void savePreference(THEME_KEY, theme).catch((error) => console.warn('[xaim] 主题持久化失败:', error))
    applyTheme(theme)
    set({ theme })
  },
}))
