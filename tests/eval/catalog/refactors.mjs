export default [
  {
    id: '016-refactor-ts-date', language: 'TS', category: 'refactor', title: '提取日期格式化辅助函数', description: '将重复的日期格式化逻辑提取到 src/date.ts 的 formatDate；app.ts 和 report.ts 必须共同调用该函数，输出保持一致。',
    files: {
      'src/app.ts': 'export function appDate(date: Date): string {\n  return date.toISOString().slice(0, 10);\n}\n',
      'src/report.ts': 'export function reportDate(date: Date): string {\n  return date.toISOString().slice(0, 10);\n}\n',
      'tests/date.test.mjs': `import assert from 'node:assert/strict';
import test from 'node:test';
import { appDate } from '../dist/app.js';
import { reportDate } from '../dist/report.js';
test('shared behavior', () => { const d = new Date('2024-01-02T03:04:05Z'); assert.equal(appDate(d), '2024-01-02'); assert.equal(reportDate(d), '2024-01-02'); });
`,
    },
    solution: { 'src/date.ts': 'export function formatDate(date: Date): string { return date.toISOString().slice(0, 10); }\n', 'src/app.ts': 'import { formatDate } from "./date.js";\nexport function appDate(date: Date): string { return formatDate(date); }\n', 'src/report.ts': 'import { formatDate } from "./date.js";\nexport function reportDate(date: Date): string { return formatDate(date); }\n' },
  },
  {
    id: '017-refactor-python-validator', language: 'Python', category: 'refactor', title: '提取 Python 验证器', description: '把 user.py 与 order.py 中重复的非空字符串检查提取到 validators.py 的 require_text；原函数行为保持一致，空白字符串必须被拒绝。',
    files: {
      'user.py': 'def valid_user(name):\n    return isinstance(name, str) and bool(name.strip())\n',
      'order.py': 'def valid_order(reference):\n    return isinstance(reference, str) and bool(reference.strip())\n',
      'tests/test_validators.py': `import unittest
from user import valid_user
from order import valid_order
class ValidatorTest(unittest.TestCase):
    def test_behavior(self):
        for value in (None, "", "  "): self.assertFalse(valid_user(value)); self.assertFalse(valid_order(value))
        for value in ("alice", "A-1"): self.assertTrue(valid_user(value)); self.assertTrue(valid_order(value))
`,
    },
    solution: { 'validators.py': 'def require_text(value):\n    return isinstance(value, str) and bool(value.strip())\n', 'user.py': 'from validators import require_text\ndef valid_user(name):\n    return require_text(name)\n', 'order.py': 'from validators import require_text\ndef valid_order(reference):\n    return require_text(reference)\n' },
  },
  {
    id: '018-refactor-rust-format', language: 'Rust', category: 'refactor', title: '提取 Rust 格式化函数', description: '将 summary.rs 和 detail.rs 的金额格式化提取到 format.rs 的 format_cents；两个调用方不得保留重复实现，输出保持一致。',
    files: {
      'src/summary.rs': 'pub fn summary(cents: i64) -> String { format!("${}.{:02}", cents / 100, cents % 100) }\n',
      'src/detail.rs': 'pub fn detail(cents: i64) -> String { format!("${}.{:02}", cents / 100, cents % 100) }\n',
      'src/lib.rs': 'pub mod summary;\npub mod detail;\n',
      'tests/format.rs': '#[test] fn values() { assert_eq!(fixture::summary::summary(123), "$1.23"); assert_eq!(fixture::detail::detail(0), "$0.00"); }\n',
    },
    solution: { 'src/format.rs': 'pub fn format_cents(cents: i64) -> String { format!("${}.{:02}", cents / 100, cents % 100) }\n', 'src/summary.rs': 'use crate::format::format_cents;\npub fn summary(cents: i64) -> String { format_cents(cents) }\n', 'src/detail.rs': 'use crate::format::format_cents;\npub fn detail(cents: i64) -> String { format_cents(cents) }\n', 'src/lib.rs': 'pub mod format;\npub mod summary;\npub mod detail;\n' },
  },
  {
    id: '019-refactor-go-normalize', language: 'Go', category: 'refactor', title: '提取 Go 标准化函数', description: '将 user.go 和 team.go 的 strings.TrimSpace + strings.ToLower 逻辑提取到 normalize.go 的 Normalize；行为保持一致。',
    files: {
      'user.go': 'package fixture\nimport "strings"\nfunc UserName(value string) string { return strings.ToLower(strings.TrimSpace(value)) }\n',
      'team.go': 'package fixture\nimport "strings"\nfunc TeamName(value string) string { return strings.ToLower(strings.TrimSpace(value)) }\n',
      'normalize_test.go': `package fixture
import "testing"
func TestNames(t *testing.T) { if UserName(" Alice ") != "alice" || TeamName(" Team ") != "team" { t.Fatal("bad normalization") } }
`,
    },
    solution: { 'normalize.go': 'package fixture\nimport "strings"\nfunc Normalize(value string) string { return strings.ToLower(strings.TrimSpace(value)) }\n', 'user.go': 'package fixture\nfunc UserName(value string) string { return Normalize(value) }\n', 'team.go': 'package fixture\nfunc TeamName(value string) string { return Normalize(value) }\n' },
  },
  {
    id: '020-refactor-python-settings', language: 'Python', category: 'refactor', title: '封装 Python 设置对象', description: '将 app.py 与 worker.py 对 TIMEOUT 的直接全局读取改为从 settings.Settings.timeout 读取；值保持 30，不能复制第二份常量。',
    files: {
      'app.py': 'TIMEOUT = 30\ndef app_timeout():\n    return TIMEOUT\n',
      'worker.py': 'TIMEOUT = 30\ndef worker_timeout():\n    return TIMEOUT\n',
      'tests/test_settings.py': `import unittest
from app import app_timeout
from worker import worker_timeout
class SettingsTest(unittest.TestCase):
    def test_defaults(self): self.assertEqual(app_timeout(), 30); self.assertEqual(worker_timeout(), 30)
`,
    },
    solution: { 'settings.py': 'class Settings:\n    timeout = 30\n', 'app.py': 'from settings import Settings\ndef app_timeout():\n    return Settings.timeout\n', 'worker.py': 'from settings import Settings\ndef worker_timeout():\n    return Settings.timeout\n' },
  },
];
