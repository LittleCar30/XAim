import assert from 'node:assert/strict';
import test from 'node:test';
import { read } from '../dist/reader.js'; import { write } from '../dist/writer.js';
test('result', () => { assert.deepEqual(read('a'), { ok: true, value: 'a' }); assert.deepEqual(write('b'), { ok: true, value: 'b' }); });
