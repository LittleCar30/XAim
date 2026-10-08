import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { copyRepo, run, verify } from './verify.mjs';

const root = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const mode = args[0];
if (!['baseline', 'reference', 'candidate'].includes(mode)) {
  console.error('Usage: node tests/eval/run.mjs baseline|reference|candidate [candidate-root] [--output report.json]');
  process.exit(2);
}
const outputIndex = args.indexOf('--output');
const candidateRoot = mode === 'candidate' ? args[1] : null;
if (mode === 'candidate' && (!candidateRoot || candidateRoot.startsWith('--'))) {
  console.error('candidate requires a directory containing <task-id>/repo/ for all 30 tasks');
  process.exit(2);
}
const taskDirs = readdirSync(path.join(root, 'tasks')).sort();
if (taskDirs.length !== 30) throw new Error(`Expected 30 tasks, found ${taskDirs.length}`);
mkdirSync(path.join(root, '.runs'), { recursive: true });
const workspace = mkdtempSync(path.join(root, '.runs', `${mode}-`));
const results = [];
for (const id of taskDirs) {
  const taskDir = path.join(root, 'tasks', id);
  const spec = JSON.parse(readFileSync(path.join(taskDir, 'task.json'), 'utf8'));
  const repo = mode === 'candidate' ? path.resolve(candidateRoot, id, 'repo') : path.join(workspace, id, 'repo');
  const started = Date.now();
  let result;
  try {
    if (!existsSync(path.join(taskDir, 'verify.sh'))) throw new Error('Missing verify.sh');
    if (mode !== 'candidate') copyRepo(path.join(taskDir, 'repo'), repo);
    if (!existsSync(repo)) throw new Error(`Missing candidate repo: ${repo}`);
    if (mode === 'reference') {
      const patch = run(['git', 'apply', '--whitespace=error', path.join(taskDir, 'expected.diff')], repo);
      if (patch.status !== 0) throw new Error(patch.output);
    }
    const checked = await verify(taskDir, repo);
    result = { id, category: spec.category, language: spec.language, passed: true, changed: checked.changed, log: checked.log };
  } catch (error) {
    result = { id, category: spec.category, language: spec.language, passed: false, error: error.message };
  }
  result.durationMs = Date.now() - started;
  results.push(result);
  console.log(`${result.passed ? 'PASS' : 'FAIL'} ${id}${result.passed ? '' : `: ${result.error.split('\n')[0]}`}`);
}
const passed = results.filter(result => result.passed).length;
const categories = Object.fromEntries([...new Set(results.map(r => r.category))].map(category => {
  const selected = results.filter(r => r.category === category);
  const count = selected.filter(r => r.passed).length;
  return [category, { passed: count, total: selected.length, successRate: count / selected.length }];
}));
const report = { mode, generatedAt: new Date().toISOString(), workspace, passed, total: results.length, successRate: passed / results.length, categories, results };
const destination = outputIndex >= 0 ? path.resolve(args[outputIndex + 1]) : path.join(root, 'reports', `${mode}.json`);
mkdirSync(path.dirname(destination), { recursive: true });
writeFileSync(destination, `${JSON.stringify(report, null, 2)}\n`);
console.log(`Success rate: ${passed}/${results.length} (${(100 * report.successRate).toFixed(2)}%)\nReport: ${destination}`);
process.exitCode = mode === 'baseline' ? (passed === 0 ? 0 : 1) : (passed === results.length ? 0 : 1);
