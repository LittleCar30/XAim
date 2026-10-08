import assert from 'node:assert/strict';

export function check({ read }) {
  assert.match(read("src/api.ts"), /export\s+(?:async\s+)?(?:function|const)\s+request\b/);
  for (const file of ["src/client.ts"]) {
    const source = read(file).replace(/\/\/[^\n]*|\/\*[\s\S]*?\*\//g, '');
    assert.match(source, /import[\s\S]*?\brequest\b[\s\S]*?from\s+["'][^"']+["']/);
    assert.match(source, /return\s+(?:await\s+)?request\s*\(/);
    assert(!source.includes('toISOString'), 'Duplicate implementation remains');
  }
}
