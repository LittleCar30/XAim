import assert from 'node:assert/strict';
export function check({ read }) {
  const report = read('REVIEW.md');
  for (const criterion of [/P1/, /checkout\.py:3/, /\.\./, /路径穿越|path traversal/i, /外|outside|escape/i, /resolve|relative_to|根目录|root/i]) assert.match(report, criterion);
}
