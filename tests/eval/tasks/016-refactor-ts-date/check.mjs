import assert from 'node:assert/strict';

export function check({ read }) {
  assert.match(read("src/date.ts"), /export\s+(?:async\s+)?(?:function|const)\s+formatDate\b/);
  for (const file of ["src/app.ts","src/report.ts"]) {
    const source = read(file).replace(/\/\/[^\n]*|\/\*[\s\S]*?\*\//g, '');
    assert.match(source, /import[\s\S]*?\bformatDate\b[\s\S]*?from\s+["'][^"']+["']/);
    assert.match(source, /return\s+(?:await\s+)?formatDate\s*\(/);
    assert(!source.includes('toISOString'), 'Duplicate implementation remains');
  }
}
