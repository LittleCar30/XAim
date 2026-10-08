import assert from 'node:assert/strict';
import test from 'node:test';
import { appDate } from '../dist/app.js';
import { reportDate } from '../dist/report.js';
test('shared behavior', () => { const d = new Date('2024-01-02T03:04:05Z'); assert.equal(appDate(d), '2024-01-02'); assert.equal(reportDate(d), '2024-01-02'); });
