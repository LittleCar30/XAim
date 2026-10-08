import assert from 'node:assert/strict';
export function check({ trusted }) {
  trusted(['python','-c', 'from settings import Settings; import app, worker; Settings.timeout=7; assert app.app_timeout()==7; assert worker.worker_timeout()==7'], {});
}
