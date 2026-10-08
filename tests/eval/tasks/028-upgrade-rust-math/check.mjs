import assert from 'node:assert/strict';
export function check({ read, run }) {
  const result = run(['cargo','metadata','--offline','--locked','--format-version','1']);
  assert.equal(result.status, 0, result.output);
  const packages = JSON.parse(result.output).packages;
  const dependency = packages.find(p => p.name === 'eval-math');
  assert.equal(dependency.version, '2.0.0');
  assert(dependency.manifest_path.split(String.fromCharCode(92)).join('/').endsWith('/vendor/math-v2/Cargo.toml'));
}
