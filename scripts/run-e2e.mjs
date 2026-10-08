import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../', import.meta.url))

async function buildBackend() {
  let executable
  const child = spawn('cargo', [
    'test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '--lib',
    '--no-run', '--message-format=json',
  ], { cwd: root, stdio: ['ignore', 'pipe', 'inherit'] })
  createInterface({ input: child.stdout }).on('line', (line) => {
    try {
      const message = JSON.parse(line)
      if (message.reason === 'compiler-artifact' && message.profile.test && message.executable) {
        executable = message.executable
      }
      if (message.reason === 'compiler-message') process.stderr.write(message.message.rendered ?? '')
    } catch {
      process.stdout.write(`${line}\n`)
    }
  })
  await completion(child)
  if (!executable) throw new Error('Cargo did not produce a Rust test executable')
  return executable
}

function completion(child) {
  return new Promise((resolve, reject) => {
    child.once('error', reject)
    child.once('close', (code, signal) => {
      if (code === 0) resolve()
      else reject(new Error(`Process failed: ${signal ?? code}`))
    })
  })
}

try {
  const executable = await buildBackend()
  const child = spawn(process.execPath, ['node_modules/vitest/vitest.mjs', 'run', '--config', 'vitest.e2e.config.mts'], {
    cwd: root,
    stdio: 'inherit',
    env: { ...process.env, XAIM_E2E_EXECUTABLE: executable },
  })
  await completion(child)
} catch (error) {
  console.error(error.message)
  process.exitCode = 1
}
