import assert from 'node:assert/strict';
export function check({ read, trusted }) {
  for (const file of ['src/summary.rs','src/detail.rs']) { const text = read(file); assert.match(text, /format_cents\(cents\)/); assert(!text.includes('format!')); }
  trusted(['cargo','test','--offline','--locked'], { 'tests/shared.rs': '#[test] fn shared() { assert_eq!(fixture::format::format_cents(500), "$5.00"); }' });
}
