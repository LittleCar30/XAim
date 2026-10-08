import assert from 'node:assert/strict';
export function check({ read, trusted }) {
  for (const file of ['user.go','team.go']) { assert.match(read(file), /Normalize\(value\)/); assert(!read(file).includes('strings.')); }
  trusted(['go','test','./...'], { 'shared_test.go': 'package fixture\nimport "testing"\nfunc TestShared(t *testing.T) { if Normalize(" X ") != "x" { t.Fatal("shared") } }\n' });
}
