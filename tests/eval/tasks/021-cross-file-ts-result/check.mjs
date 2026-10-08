import assert from 'node:assert/strict';
export function check({ read }) {
  assert.match(read('src/result.ts'), /export\s+(?:type|interface)\s+Result\s*</);
  for (const file of ['src/reader.ts','src/writer.ts']) {
    assert.match(read(file), /import[\s\S]*?Result[\s\S]*?from\s+['"]\.\/result\.js['"]/);
    assert.match(read(file), /:\s*Result<string>/);
  }
}
