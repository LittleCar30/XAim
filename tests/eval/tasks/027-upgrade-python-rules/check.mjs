import assert from 'node:assert/strict';
export function check({ read, run }) {
  assert.equal(read('requirements.txt').trim(), './vendor/rules-v2');
  const result = run(['python','-c', 'import sys; sys.path.insert(0,".venv/lib"); import importlib.metadata; assert importlib.metadata.version("eval-rules")=="2.0.0"']);
  assert.equal(result.status, 0, result.output);
}
