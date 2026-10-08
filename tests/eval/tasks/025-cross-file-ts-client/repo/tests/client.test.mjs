import assert from 'node:assert/strict';
import test from 'node:test';
import { getUser } from '../dist/client.js';
test('client delegates', async () => { globalThis.fetch = async (url) => new Response(url); const response = await getUser('7'); assert.equal(await response.text(), '/users/7'); });
