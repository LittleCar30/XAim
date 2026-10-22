import { describe, expect, it } from 'vitest'

import { APP_NAME, APP_VERSION, fullVersion } from '@/lib/version'
import { version } from '../../package.json'
import { version as nativeVersion } from '../../src-tauri/tauri.conf.json'

describe('fullVersion', () => {
  it('formats name and version', () => {
    expect(fullVersion()).toBe('XAim v0.1.2')
  })

  it('keeps app metadata in sync', () => {
    expect(APP_NAME).toBe('XAim')
    expect(APP_VERSION).toBe(version)
    expect(APP_VERSION).toBe(nativeVersion)
  })
})
