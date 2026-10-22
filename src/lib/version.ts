import { version } from '../../package.json'

export const APP_NAME = 'XAim'
export const APP_VERSION = version

export function fullVersion(): string {
  return `${APP_NAME} v${APP_VERSION}`
}
