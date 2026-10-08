import { describe, expect, it } from 'vitest'

import { APP_NAME, APP_VERSION, fullVersion } from '@/lib/version'

describe('fullVersion', () => {
  it('formats name and version', () => {
    expect(fullVersion()).toBe('XAim v0.1')
  })

  it('keeps app metadata in sync', () => {
    expect(APP_NAME).toBe('XAim')
    expect(APP_VERSION).toBe('0.1')
  })
})
