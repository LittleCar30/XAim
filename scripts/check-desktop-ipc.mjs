// Launch the actual Windows desktop executable with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>, then run:
// node scripts/check-desktop-ipc.mjs http://127.0.0.1:<port>
// No model request or task record is created: project -1 cannot exist.
const endpoint = process.argv[2]
if (!endpoint || !/^http:\/\/127\.0\.0\.1:\d+$/.test(endpoint)) {
  throw new Error('Provide the local WebView debugging endpoint')
}
const pages = await (await fetch(`${endpoint}/json/list`)).json()
const page = pages.find((item) => item.type === 'page' && /tauri\.localhost|localhost:5173/.test(item.url))
if (!page) throw new Error('XAim desktop WebView not found')
const socket = new WebSocket(page.webSocketDebuggerUrl)
try {
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true })
    socket.addEventListener('error', reject, { once: true })
  })
  const result = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Desktop IPC did not respond')), 15_000)
    const fail = () => {
      clearTimeout(timer)
      reject(new Error('Desktop WebView disconnected during task invocation'))
    }
    socket.addEventListener('close', fail, { once: true })
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data)
      if (message.id !== 1) return
      clearTimeout(timer)
      socket.removeEventListener('close', fail)
      if (message.error || message.result?.exceptionDetails) {
        reject(new Error('Desktop IPC probe failed'))
      } else resolve(message.result.result.value)
    })
    socket.send(JSON.stringify({
      id: 1, method: 'Runtime.evaluate', params: {
        awaitPromise: true, returnByValue: true,
        expression: `(async () => {
          for (let i = 0; i < 3; i++) {
            try {
              await window.__TAURI_INTERNALS__.invoke('run_task', {
                projectId: -1, prompt: 'desktop stack regression', allowVerification: false
              });
              throw new Error('Unexpected task success');
            } catch (error) {
              if (!String(error).includes('项目 #-1 不存在')) throw new Error('Unexpected task rejection');
            }
          }
          const providers = await window.__TAURI_INTERNALS__.invoke('list_providers');
          return { repeatedTaskRejections: 3, desktopResponsive: Array.isArray(providers) && providers.length > 0 };
        })()`,
      },
    }))
  })
  if (!result.desktopResponsive) throw new Error('Desktop did not remain responsive')
  console.log(JSON.stringify(result))
} finally {
  socket.close()
}
