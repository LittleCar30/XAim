import { average } from '../dist/average.js';
import assert from 'node:assert/strict';
import test from 'node:test';
test('empty', () => assert.equal(average([]), 0));
test('numbers', () => { assert.equal(average([1, 2, 6]), 3); assert.equal(average([-2, 0]), -1); assert.equal(average([0.5, 1.5]), 1); });
