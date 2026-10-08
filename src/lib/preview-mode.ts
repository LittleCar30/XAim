export const isPreview = import.meta.env.DEV &&
  new URLSearchParams(window.location.search).get('preview') === '1' &&
  !('__TAURI_INTERNALS__' in window)
