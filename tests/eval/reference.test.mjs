import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { applyReferencePatch } from './reference.mjs';
import { run } from './verify.mjs';

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'xaim-reference-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  assert.equal(run(['git', 'init', '--quiet'], root).status, 0);
  const repo = path.join(root, '.runs', 'reference', 'repo');
  mkdirSync(repo, { recursive: true });
  writeFileSync(path.join(root, 'file.txt'), 'parent\n');
  writeFileSync(path.join(repo, 'file.txt'), 'before\n');
  return { root, repo, patch: path.join(root, 'expected.diff') };
}

test('reference patches modify and create files inside a nested candidate only', t => {
  const { root, repo, patch } = fixture(t);
  writeFileSync(patch, [
    'diff --git a/file.txt b/file.txt',
    '--- a/file.txt', '+++ b/file.txt', '@@ -1 +1 @@', '-before', '+after',
    'diff --git a/new.txt b/new.txt', 'new file mode 100644',
    '--- /dev/null', '+++ b/new.txt', '@@ -0,0 +1 @@', '+created', '',
  ].join('\n'));
  applyReferencePatch(repo, patch);
  assert.equal(readFileSync(path.join(repo, 'file.txt'), 'utf8'), 'after\n');
  assert.equal(readFileSync(path.join(repo, 'new.txt'), 'utf8'), 'created\n');
  assert.equal(readFileSync(path.join(root, 'file.txt'), 'utf8'), 'parent\n');
  assert.equal(existsSync(path.join(root, 'new.txt')), false);
});

test('reference patch failures are reported instead of silently skipped', t => {
  const { repo, patch } = fixture(t);
  writeFileSync(patch, [
    'diff --git a/file.txt b/file.txt', '--- a/file.txt', '+++ b/file.txt',
    '@@ -1 +1 @@', '-does not match', '+after', '',
  ].join('\n'));
  assert.throws(() => applyReferencePatch(repo, patch), /patch/i);
  assert.equal(readFileSync(path.join(repo, 'file.txt'), 'utf8'), 'before\n');
});
