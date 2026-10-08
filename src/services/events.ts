import { listen, UnlistenFn } from '@tauri-apps/api/event';
import type { XaimEvent } from '../types';

/**
 * 订阅后端事件流。通道名与 src-tauri/src/events.rs 的 EVENT_CHANNEL 保持一致，
 * payload 为 XaimEvent 判别联合，调用方按 `type` 字段分发给各 store。
 */
export async function subscribeEvents(
  handler: (event: XaimEvent) => void
): Promise<UnlistenFn> {
  return listen<XaimEvent>('xaim://event', (e) => handler(e.payload));
}
