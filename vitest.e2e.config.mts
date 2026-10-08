import { defineConfig } from 'vitest/config'
import baseConfig from './vite.config.mjs'

export default defineConfig({
  ...baseConfig,
  test: {
    ...baseConfig.test,
    include: ['e2e/**/*.test.tsx'],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
    coverage: { enabled: false },
  },
})
