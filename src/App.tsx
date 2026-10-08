import { useEffect, useState } from 'react'
import type { UnlistenFn } from '@tauri-apps/api/event'

import Settings from '@/pages/Settings'
import Workspace from '@/pages/Workspace'
import { subscribeEvents } from '@/services/events'
import { useTaskStore } from '@/stores/task'
import { applyTheme, useThemeStore } from '@/stores/theme'
import { isPreview } from '@/lib/preview-mode'

/** 顶层路由。桌面端无需 history 路由，用 hash 即可（#/ 与 #/settings）。 */
export type Route = 'workspace' | 'settings'

function routeFromHash(): Route {
  return window.location.hash.startsWith('#/settings') ? 'settings' : 'workspace'
}

function navigate(route: Route) {
  window.location.hash = route === 'settings' ? '#/settings' : '#/'
}

export default function App() {
  const [route, setRoute] = useState<Route>(routeFromHash)
  const theme = useThemeStore((s) => s.theme)

  useEffect(() => {
    applyTheme(theme)
  }, [theme])

  // 订阅后端事件流，分发到任务 store；App 卸载时退订。
  useEffect(() => {
    if (isPreview) return
    let disposed = false
    let unlisten: UnlistenFn | undefined

    subscribeEvents((event) => useTaskStore.getState().handleEvent(event))
      .then((fn) => {
        // StrictMode 下首挂载立即清理时，订阅可能尚未完成，直接退订。
        if (disposed) fn()
        else unlisten = fn
      })
      .catch((err) => {
        // 非 Tauri 环境（纯浏览器 dev）下 listen 必然失败，降级为警告。
        console.warn('[xaim] 事件流订阅失败（非 Tauri 环境可忽略）:', err)
      })

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  // hash 路由：跟随浏览器前进/后退与手动改地址。
  useEffect(() => {
    const onHashChange = () => setRoute(routeFromHash())
    window.addEventListener('hashchange', onHashChange)
    return () => window.removeEventListener('hashchange', onHashChange)
  }, [])

  return route === 'settings' ? (
    <Settings onBack={() => navigate('workspace')} />
  ) : (
    <Workspace onOpenSettings={() => navigate('settings')} />
  )
}
