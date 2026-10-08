import assert from 'node:assert/strict';
import test from 'node:test';
import { retry } from '../dist/retry.js';
test('immediate success', () => assert.equal(retry(() => true, 2), true));
