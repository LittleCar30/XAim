import assert from 'node:assert/strict';
export function check({ read, run }) {
  const pkg = JSON.parse(read('package.json')), lock = JSON.parse(read('package-lock.json'));
  assert.equal(pkg.dependencies['slug-kit'], 'file:vendor/slug-v2');
  assert.equal(lock.packages[''].dependencies['slug-kit'], pkg.dependencies['slug-kit']);
  assert.equal(lock.packages['vendor/slug-v2'].version, '2.0.0');
  const result = run(['node','--input-type=module','-e', 'import {slugify} from "slug-kit"; if(slugify("A B")!=="a-b") process.exit(1)']);
  assert.equal(result.status, 0, result.output);
}
