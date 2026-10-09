import assert from 'node:assert/strict';
export function check({ read }) {
  const report = read('REVIEW.md');
  for (const criterion of [/P1/, /src\/api\.ts\x60?\s*(?::\s*2(?!\d)|第\s*2\s*行|(?:[（(]\s*)?line\s+2\b)/i, /网络|network|reject|断开/i, /catch|吞|swallow/i, /诊断|上下文|context|diagnos/i, /抛|Result|throw|error/i]) assert.match(report, criterion);
}
