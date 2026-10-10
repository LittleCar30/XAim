import { ipc } from '@/services/ipc'

const KEYS = ['xaim.theme', 'xaim.settings-prefs', 'xaim.prompt-history']

/** WebView is a fast cache; the database travels with backups and data migrations. */
export async function savePreference(key: string, value: string): Promise<void> {
  try { localStorage.setItem(key, value) } catch { /* The database remains available. */ }
  await ipc.setPreference(key, value)
}

export async function hydratePreferences(): Promise<void> {
  const persisted = await ipc.getPreferences()
  for (const key of KEYS) {
    if (persisted[key] !== undefined) {
      try { localStorage.setItem(key, persisted[key]) } catch { /* Cache is optional. */ }
    } else {
      let previous: string | null = null
      try { previous = localStorage.getItem(key) } catch { /* First run without a cache. */ }
      if (previous !== null) await ipc.setPreference(key, previous)
    }
  }
}
