import { query } from '../dist/query.js';
import assert from 'node:assert/strict';
import test from 'node:test';
test('query encoding', () => {
  const result = query({ 'a&b': 'x=y', q: '你好 world' });
  const parsed = new URLSearchParams(result);
  assert.equal(parsed.get('a&b'), 'x=y'); assert.equal(parsed.get('q'), '你好 world');
  assert.equal([...parsed].length, 2); assert.equal(query({}), '');
});
