import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const ignored = new Set(['node_modules', 'dist', 'target', '__pycache__', '.pytest_cache', '.git', '.venv', 'build']);
export function files(root, prefix = '') {
  return readdirSync(path.join(root, prefix), { withFileTypes: true }).flatMap(entry => {
    if (ignored.has(entry.name) || entry.name.endsWith('.pyc') || entry.name.endsWith('.egg-info')) return [];
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    return entry.isDirectory() ? files(root, name) : [name];
  }).sort();
}

export function run(command, cwd) {
  const actual = [...command];
  if (actual[0] === 'python') actual[0] = process.env.EVAL_PYTHON || 'python';
  if (actual[0] === 'go') actual[0] = process.env.EVAL_GO || 'go';
  if (actual[0] === 'npm' && process.platform === 'win32') {
    actual.splice(0, 1, process.env.ComSpec || 'cmd.exe', '/d', '/s', '/c', 'npm.cmd');
  }
  const result = spawnSync(actual[0], actual.slice(1), {
    cwd, encoding: 'utf8', timeout: 120_000,
    env: { ...process.env, GOWORK: 'off', GOTOOLCHAIN: 'local', PYTHONDONTWRITEBYTECODE: '1' },
    maxBuffer: 8 * 1024 * 1024,
  });
  return { status: result.status, output: `${result.stdout || ''}${result.stderr || ''}${result.error ? result.error.message : ''}` };
}

export function copyRepo(source, target) {
  cpSync(source, target, { recursive: true, filter: name => !ignored.has(path.basename(name)) && !name.endsWith('.pyc') && !name.endsWith('.egg-info') });
}

export async function verify(taskDir, repoDir, { prepare = true } = {}) {
  const task = JSON.parse(readFileSync(path.join(taskDir, 'task.json'), 'utf8'));
  const baseline = path.join(taskDir, 'repo');
  const original = files(baseline);
  const candidate = files(repoDir);
  const changed = [...new Set([...original, ...candidate])].filter(name => {
    const before = path.join(baseline, name), after = path.join(repoDir, name);
    return !existsSync(before) || !existsSync(after) || !readFileSync(before).equals(readFileSync(after));
  });
  for (const name of changed) assert(task.allowed.includes(name), `Forbidden file change: ${name}`);
  let log = '';
  const execute = command => {
    const result = run(command, repoDir);
    log += `$ ${command.join(' ')}\n${result.output}\n`;
    assert.equal(result.status, 0, log);
  };
  if (prepare) for (const command of task.prepare || []) execute(command);
  for (const command of task.commands) execute(command);
  const hook = path.join(taskDir, 'check.mjs');
  if (existsSync(hook)) {
    const { check } = await import(pathToFileURL(hook).href);
    const trusted = (command, extraFiles) => {
      const scratch = mkdtempSync(path.join(tmpdir(), 'xaim-eval-trusted-'));
      try {
        copyRepo(repoDir, scratch);
        for (const [name, content] of Object.entries(extraFiles)) {
          const destination = path.join(scratch, name);
          mkdirSync(path.dirname(destination), { recursive: true });
          writeFileSync(destination, content);
        }
        const result = run(command, scratch);
        assert.equal(result.status, 0, result.output);
      } finally {
        rmSync(scratch, { recursive: true, force: true });
      }
    };
    await check({ repo: repoDir, read: name => readFileSync(path.join(repoDir, name), 'utf8'), run: command => run(command, repoDir), trusted });
  }
  for (const mutation of task.mutations || []) {
    const scratch = mkdtempSync(path.join(tmpdir(), 'xaim-eval-mutant-'));
    try {
      copyRepo(repoDir, scratch);
      if (existsSync(path.join(repoDir, 'node_modules'))) {
        symlinkSync(path.join(repoDir, 'node_modules'), path.join(scratch, 'node_modules'), process.platform === 'win32' ? 'junction' : 'dir');
      }
      const name = path.join(scratch, mutation.file);
      const source = readFileSync(name, 'utf8');
      assert(source.includes(mutation.before), `Mutation anchor missing: ${mutation.file}`);
      writeFileSync(name, source.replace(mutation.before, mutation.after));
      const result = run(mutation.command, scratch);
      assert(result.status !== null && result.status > 0, `Tests did not catch mutant: ${mutation.name}\n${result.output}`);
      assert(result.output.includes(mutation.failure), `Mutant must fail an assertion: ${mutation.name}\n${result.output}`);
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }
  for (const name of task.required) assert(changed.includes(name), `Required change missing: ${name}`);
  return { changed, log };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const taskDir = path.resolve(process.argv[2]);
  const repoDir = path.resolve(process.argv[3] || path.join(taskDir, 'repo'));
  try {
    const result = await verify(taskDir, repoDir);
    process.stdout.write(`${result.log}PASS ${path.basename(taskDir)}\n`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
