import assert from 'node:assert/strict';
import test from 'node:test';
import { discount } from '../dist/discount.js';
test('ordinary discount', () => assert.equal(discount(100, 0.2), 80));
