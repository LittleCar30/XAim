import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { hooks } from './catalog/hooks.mjs';
import { check as checkout } from './tasks/029-review-checkout/check.mjs';
import { check as api } from './tasks/030-review-api/check.mjs';

test('equivalent review locators pass and inaccurate locations fail', () => {
  for (const [check, file, line, finding] of [
    [checkout, 'checkout.py', 3, 'P1 ../secret 路径穿越读取外部文件，resolve 后检查 root'],
    [api, 'src/api.ts', 2, 'P1 network 断开时 catch 吞错误，丢失 context；throw error'],
  ]) {
    for (const location of [`${file}:${line}`, `\`${file}\` 第 ${line} 行`, `${file} (line ${line})`]) {
      assert.doesNotThrow(() => check({ read: () => `${location}: ${finding}` }));
    }
    for (const location of [`${file}:${line + 1}`, `${file} 第 ${line + 1} 行`, `${file}:${line}0`]) {
      assert.throws(() => check({ read: () => `${location}: ${finding}` }));
    }
    assert.throws(() => check({ read: () => `${file}:${line}: P1` }));
  }
});

test('standalone validators match the task generator', () => {
  for (const id of ['029-review-checkout', '030-review-api']) {
    assert.equal(readFileSync(new URL(`./tasks/${id}/check.mjs`, import.meta.url), 'utf8').replaceAll('\r\n', '\n'), hooks[id].replaceAll('\r\n', '\n'));
  }
});
