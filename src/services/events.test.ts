import { beforeEach, describe, expect, it, vi } from 'vitest'

import { listen } from '@tauri-apps/api/event'
import { subscribeEvents } from '@/services/events'
import type { XaimEvent } from '@/types'

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}))

const mockedListen = vi.mocked(listen)

describe('subscribeEvents', () => {
  const unlisten = vi.fn()
  const handler = vi.fn()

  beforeEach(() => {
    vi.clearAllMocks()
    mockedListen.mockResolvedValue(unlisten)
  })

  it('subscribes on the backend event channel', async () => {
    await subscribeEvents(handler)
    expect(mockedListen).toHaveBeenCalledTimes(1)
    expect(mockedListen).toHaveBeenCalledWith('xaim://event', expect.any(Function))
  })

  it('forwards the event payload to the handler', async () => {
    await subscribeEvents(handler)
    const onEvent = mockedListen.mock.calls[0][1]
    const payload: XaimEvent = {
      type: 'step',
      taskId: 1,
      name: '生成补丁',
      status: 'running',
    }
    onEvent({ event: 'xaim://event', id: 0, payload })
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler).toHaveBeenCalledWith(payload)
  })

  it('returns the unlisten function', async () => {
    const result = await subscribeEvents(handler)
    expect(result).toBe(unlisten)
  })
})
