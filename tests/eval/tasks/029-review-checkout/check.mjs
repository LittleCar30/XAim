import assert from 'node:assert/strict';
export function check({ read }) {
  const report = read('REVIEW.md');
  for (const criterion of [/P1/, /checkout\.py\x60?\s*(?::\s*3(?!\d)|第\s*3\s*行|(?:[（(]\s*)?line\s+3\b)/i, /\.\./, /路径穿越|path traversal/i, /外|outside|escape/i, /resolve|relative_to|根目录|root/i]) assert.match(report, criterion);
}
