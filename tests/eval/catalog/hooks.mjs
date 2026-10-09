const preamble = `import assert from 'node:assert/strict';
`;
const tsShared = (helperFile, symbol, callers) => preamble + String.raw`
export function check({ read }) {
  assert.match(read(${JSON.stringify(helperFile)}), /export\s+(?:async\s+)?(?:function|const)\s+${symbol}\b/);
  for (const file of ${JSON.stringify(callers)}) {
    const source = read(file).replace(/\/\/[^\n]*|\/\*[\s\S]*?\*\//g, '');
    assert.match(source, /import[\s\S]*?\b${symbol}\b[\s\S]*?from\s+["'][^"']+["']/);
    assert.match(source, /return\s+(?:await\s+)?${symbol}\s*\(/);
    assert(!source.includes('toISOString'), 'Duplicate implementation remains');
  }
}
`;
export const hooks = {
  '016-refactor-ts-date': tsShared('src/date.ts', 'formatDate', ['src/app.ts','src/report.ts']),
  '017-refactor-python-validator': preamble + String.raw`export function check({ trusted }) {
  trusted(['python','-c', 'import validators, user, order; validators.require_text = lambda value: "sentinel"; user.require_text = validators.require_text; order.require_text = validators.require_text; assert user.valid_user(None) == "sentinel"; assert order.valid_order(None) == "sentinel"'], {});
  trusted(['python','-c', 'import ast; from pathlib import Path; assert all(any(isinstance(n, ast.ImportFrom) and n.module == "validators" for n in ast.walk(ast.parse(Path(p).read_text()))) for p in ["user.py","order.py"])'], {});
}
`,
  '018-refactor-rust-format': preamble + String.raw`export function check({ read, trusted }) {
  for (const file of ['src/summary.rs','src/detail.rs']) { const text = read(file); assert.match(text, /format_cents\(cents\)/); assert(!text.includes('format!')); }
  trusted(['cargo','test','--offline','--locked'], { 'tests/shared.rs': '#[test] fn shared() { assert_eq!(fixture::format::format_cents(500), "$5.00"); }' });
}
`,
  '019-refactor-go-normalize': preamble + String.raw`export function check({ read, trusted }) {
  for (const file of ['user.go','team.go']) { assert.match(read(file), /Normalize\(value\)/); assert(!read(file).includes('strings.')); }
  trusted(['go','test','./...'], { 'shared_test.go': 'package fixture\nimport "testing"\nfunc TestShared(t *testing.T) { if Normalize(" X ") != "x" { t.Fatal("shared") } }\n' });
}
`,
  '020-refactor-python-settings': preamble + String.raw`export function check({ trusted }) {
  trusted(['python','-c', 'from settings import Settings; import app, worker; Settings.timeout=7; assert app.app_timeout()==7; assert worker.worker_timeout()==7'], {});
}
`,
  '021-cross-file-ts-result': preamble + String.raw`export function check({ read }) {
  assert.match(read('src/result.ts'), /export\s+(?:type|interface)\s+Result\s*</);
  for (const file of ['src/reader.ts','src/writer.ts']) {
    assert.match(read(file), /import[\s\S]*?Result[\s\S]*?from\s+['"]\.\/result\.js['"]/);
    assert.match(read(file), /:\s*Result<string>/);
  }
}
`,
  '022-cross-file-python-model': preamble + String.raw`export function check({ trusted }) {
  trusted(['python','-c', 'import models, service, serializer; from dataclasses import is_dataclass; assert is_dataclass(models.User); assert service.User is models.User; assert serializer.User is models.User; assert service.greet(models.User("B"))=="Hello B"; assert serializer.serialize(models.User("B"))=={"name":"B"}'], {});
}
`,
  '023-cross-file-rust-error': preamble + String.raw`export function check({ trusted }) {
  trusted(['cargo','test','--offline','--locked'], { 'tests/shared_error.rs': '#[test] fn shared() { let _: Result<String, fixture::error::AppError> = fixture::reader::read(""); let _: Result<String, fixture::error::AppError> = fixture::writer::write(""); }\n' });
}
`,
  '024-cross-file-go-config': preamble + String.raw`export function check({ trusted }) {
  trusted(['go','test','./...'], { 'config_contract_test.go': 'package fixture\nimport "testing"\nfunc TestConfigContract(t *testing.T) { t.Setenv("PORT", "9001"); var cfg Config = LoadConfig(); if cfg.Port != 9001 || Port() != 9001 { t.Fatal("configuration ignored") }; t.Setenv("PORT", "bad"); if LoadConfig().Port != 8080 { t.Fatal("invalid default") } }\n' });
}
`,
  '025-cross-file-ts-client': tsShared('src/api.ts', 'request', ['src/client.ts']),
  '026-upgrade-ts-slug-kit': preamble + String.raw`export function check({ read, run }) {
  const pkg = JSON.parse(read('package.json')), lock = JSON.parse(read('package-lock.json'));
  assert.equal(pkg.dependencies['slug-kit'], 'file:vendor/slug-v2');
  assert.equal(lock.packages[''].dependencies['slug-kit'], pkg.dependencies['slug-kit']);
  assert.equal(lock.packages['vendor/slug-v2'].version, '2.0.0');
  const result = run(['node','--input-type=module','-e', 'import {slugify} from "slug-kit"; if(slugify("A B")!=="a-b") process.exit(1)']);
  assert.equal(result.status, 0, result.output);
}
`,
  '027-upgrade-python-rules': preamble + String.raw`export function check({ read, run }) {
  assert.equal(read('requirements.txt').trim(), './vendor/rules-v2');
  const result = run(['python','-c', 'import sys; sys.path.insert(0,".venv/lib"); import importlib.metadata; assert importlib.metadata.version("eval-rules")=="2.0.0"']);
  assert.equal(result.status, 0, result.output);
}
`,
  '028-upgrade-rust-math': preamble + String.raw`export function check({ read, run }) {
  const result = run(['cargo','metadata','--offline','--locked','--format-version','1']);
  assert.equal(result.status, 0, result.output);
  const packages = JSON.parse(result.output).packages;
  const dependency = packages.find(p => p.name === 'eval-math');
  assert.equal(dependency.version, '2.0.0');
  assert(dependency.manifest_path.split(String.fromCharCode(92)).join('/').endsWith('/vendor/math-v2/Cargo.toml'));
}
`,
  '029-review-checkout': preamble + String.raw`export function check({ read }) {
  const report = read('REVIEW.md');
  for (const criterion of [/P1/, /checkout\.py\x60?\s*(?::\s*3(?!\d)|第\s*3\s*行|(?:[（(]\s*)?line\s+3\b)/i, /\.\./, /路径穿越|path traversal/i, /外|outside|escape/i, /resolve|relative_to|根目录|root/i]) assert.match(report, criterion);
}
`,
  '030-review-api': preamble + String.raw`export function check({ read }) {
  const report = read('REVIEW.md');
  for (const criterion of [/P1/, /src\/api\.ts\x60?\s*(?::\s*2(?!\d)|第\s*2\s*行|(?:[（(]\s*)?line\s+2\b)/i, /网络|network|reject|断开/i, /catch|吞|swallow/i, /诊断|上下文|context|diagnos/i, /抛|Result|throw|error/i]) assert.match(report, criterion);
}
`,
};
