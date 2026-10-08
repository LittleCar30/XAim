export default [
  {
    id: '001-fix-null-pointer', language: 'TS', category: 'bugfix', title: '修复登录空指针',
    description: '修复 src/auth.ts 的 login；null 或 undefined 用户返回 null，有效用户返回 name，空字符串必须保留。只修改 src/auth.ts。',
    files: {
      'src/auth.ts': 'export interface User {\n  name: string;\n}\n\nexport function login(user: User | null | undefined): string | null {\n  return (user as User).name;\n}\n',
      'tests/auth.test.mjs': 'import assert from "node:assert/strict";\nimport test from "node:test";\nimport { login } from "../dist/auth.js";\ntest("valid", () => assert.equal(login({name: "Alice"}), "Alice"));\ntest("empty name", () => assert.equal(login({name: ""}), ""));\ntest("null", () => assert.equal(login(null), null));\ntest("undefined", () => assert.equal(login(undefined), null));\n',
    },
    solution: { 'src/auth.ts': 'export interface User {\n  name: string;\n}\n\nexport function login(user: User | null | undefined): string | null {\n  return user?.name ?? null;\n}\n' },
  },
  {
    id: '002-fix-zero-average', language: 'TS', category: 'bugfix',
    title: '修复空数组平均值', description: 'src/average.ts 的 average([]) 应返回 0；非空数组仍返回算术平均值，包含负数和小数。',
    files: {
      'src/average.ts': 'export function average(values: number[]): number {\n  return values.reduce((sum, value) => sum + value, 0) / values.length;\n}\n',
      'tests/average.test.mjs': `import { average } from '../dist/average.js';
import assert from 'node:assert/strict';
import test from 'node:test';
test('empty', () => assert.equal(average([]), 0));
test('numbers', () => { assert.equal(average([1, 2, 6]), 3); assert.equal(average([-2, 0]), -1); assert.equal(average([0.5, 1.5]), 1); });
`,
    },
    solution: { 'src/average.ts': 'export function average(values: number[]): number {\n  if (values.length === 0) return 0;\n  return values.reduce((sum, value) => sum + value, 0) / values.length;\n}\n' },
  },
  {
    id: '003-fix-python-pagination', language: 'Python', category: 'bugfix',
    title: '修复分页尾部数据丢失', description: 'pagination.py 的 pages(items, size) 应包含最后不足 size 的一页；空输入返回空列表。size <= 0 抛出 ValueError。',
    files: {
      'pagination.py': 'def pages(items, size):\n    if size <= 0:\n        raise ValueError("size must be positive")\n    return [items[i:i + size] for i in range(0, len(items) - size + 1, size)]\n',
      'tests/test_pagination.py': `import unittest
from pagination import pages
class PaginationTest(unittest.TestCase):
    def test_tail(self):
        self.assertEqual(pages([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]])
        self.assertEqual(pages([1], 3), [[1]])
    def test_empty_exact_invalid(self):
        self.assertEqual(pages([], 2), [])
        self.assertEqual(pages([1, 2], 2), [[1, 2]])
        for size in (0, -1):
            with self.assertRaises(ValueError): pages([1], size)
`,
    },
    solution: { 'pagination.py': 'def pages(items, size):\n    if size <= 0:\n        raise ValueError("size must be positive")\n    return [items[i:i + size] for i in range(0, len(items), size)]\n' },
  },
  {
    id: '004-fix-python-mutable-default', language: 'Python', category: 'bugfix',
    title: '修复默认容器共享', description: 'cache.py 的 add(item, bucket) 在未传 bucket 时每次创建独立列表；显式传入 bucket 时追加到原列表并返回它。',
    files: {
      'cache.py': 'def add(item, bucket=[]):\n    bucket.append(item)\n    return bucket\n',
      'tests/test_cache.py': `import unittest
from cache import add
class CacheTest(unittest.TestCase):
    def test_isolation(self):
        first = add("a")
        second = add("b")
        self.assertEqual(first, ["a"])
        self.assertEqual(second, ["b"])
        self.assertIsNot(first, second)
    def test_explicit_bucket(self):
        bucket = []
        self.assertIs(add("c", bucket), bucket)
        self.assertEqual(bucket, ["c"])
`,
    },
    solution: { 'cache.py': 'def add(item, bucket=None):\n    if bucket is None:\n        bucket = []\n    bucket.append(item)\n    return bucket\n' },
  },
  {
    id: '005-fix-rust-range-index', language: 'Rust', category: 'bugfix',
    title: '修复 Rust 索引越界', description: 'src/lib.rs 的 total 应正确求和，空切片返回 0，不得 panic。',
    files: {
      'src/lib.rs': `pub fn total(items: &[i32]) -> i32 {
    let mut result = 0;
    for index in 0..=items.len() {
        result += items[index];
    }
    result
}
`,
      'tests/total.rs': `use fixture::total;
#[test] fn empty() { assert_eq!(total(&[]), 0); }
#[test] fn numbers() { assert_eq!(total(&[1, 2, 3]), 6); assert_eq!(total(&[-2, 1]), -1); }
`,
    },
    solution: { 'src/lib.rs': `pub fn total(items: &[i32]) -> i32 {
    items.iter().sum()
}
` },
  },
  {
    id: '006-fix-rust-invalid-port', language: 'Rust', category: 'bugfix',
    title: '修复端口解析崩溃', description: 'src/lib.rs 的 port 对合法 u16 数字返回其值；无效、负数、溢出或空输入返回默认端口 8080。合法 0 必须保留。',
    files: {
      'src/lib.rs': 'pub fn port(value: &str) -> u16 {\n    value.parse::<u16>().unwrap()\n}\n',
      'tests/port.rs': `use fixture::port;
#[test] fn valid() { assert_eq!(port("0"), 0); assert_eq!(port("443"), 443); assert_eq!(port("65535"), 65535); }
#[test] fn invalid() { for input in ["", "nope", "-1", "65536"] { assert_eq!(port(input), 8080); } }
`,
    },
    solution: { 'src/lib.rs': 'pub fn port(value: &str) -> u16 {\n    value.parse::<u16>().unwrap_or(8080)\n}\n' },
  },
  {
    id: '007-fix-go-empty-first', language: 'Go', category: 'bugfix',
    title: '修复空切片访问', description: 'first.go 的 First 对 nil 或空切片返回 (0, false)，非空输入返回 (首元素, true)，首元素为 0 也应返回 true。',
    files: {
      'first.go': 'package fixture\n\nfunc First(values []int) (int, bool) {\n    return values[0], true\n}\n',
      'first_test.go': `package fixture
import "testing"
func TestFirst(t *testing.T) {
    for _, values := range [][]int{nil, {}, {0}, {4, 5}} {
        got, ok := First(values)
        if len(values) == 0 { if got != 0 || ok { t.Fatalf("empty: %d %v", got, ok) } } else if got != values[0] || !ok { t.Fatal("wrong first value") }
    }
}
`,
    },
    solution: { 'first.go': 'package fixture\n\nfunc First(values []int) (int, bool) {\n    if len(values) == 0 { return 0, false }\n    return values[0], true\n}\n' },
  },
  {
    id: '008-fix-go-error-propagation', language: 'Go', category: 'bugfix',
    title: '修复忽略数字解析错误', description: 'parse.go 的 ParseCount 必须将 strconv.Atoi 的错误传给调用方，合法非负整数成功；负数返回错误。',
    files: {
      'parse.go': 'package fixture\n\nimport ("fmt"; "strconv")\nfunc ParseCount(input string) (int, error) {\n    number, _ := strconv.Atoi(input)\n    if number < 0 { return 0, fmt.Errorf("negative count") }\n    return number, nil\n}\n',
      'parse_test.go': `package fixture
import "testing"
func TestCounts(t *testing.T) {
    for _, input := range []string{"no", "", "-1", "999999999999999999999999"} { if _, err := ParseCount(input); err == nil { t.Fatalf("missing error: %q", input) } }
    for _, input := range []string{"0", "12"} { n, err := ParseCount(input); if err != nil || (input == "12" && n != 12) { t.Fatalf("valid: %d %v", n, err) } }
}
`,
    },
    solution: { 'parse.go': 'package fixture\n\nimport ("fmt"; "strconv")\nfunc ParseCount(input string) (int, error) {\n    number, err := strconv.Atoi(input)\n    if err != nil { return 0, err }\n    if number < 0 { return 0, fmt.Errorf("negative count") }\n    return number, nil\n}\n' },
  },
  {
    id: '009-fix-ts-query-encoding', language: 'TS', category: 'bugfix',
    title: '修复查询参数编码', description: 'src/query.ts 的 query 必须将参数键和值按 URL 查询组件编码；参数之间以 & 连接，不编码分隔符。空对象返回空字符串。',
    files: {
      'src/query.ts': 'export function query(values: Record<string, string>): string {\n  return Object.entries(values).map(([key, value]) => `${key}=${value}`).join("&");\n}\n',
      'tests/query.test.mjs': `import { query } from '../dist/query.js';
import assert from 'node:assert/strict';
import test from 'node:test';
test('query encoding', () => {
  const result = query({ 'a&b': 'x=y', q: '你好 world' });
  const parsed = new URLSearchParams(result);
  assert.equal(parsed.get('a&b'), 'x=y'); assert.equal(parsed.get('q'), '你好 world');
  assert.equal([...parsed].length, 2); assert.equal(query({}), '');
});
`,
    },
    solution: { 'src/query.ts': 'export function query(values: Record<string, string>): string {\n  return Object.entries(values).map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`).join("&");\n}\n' },
  },
  {
    id: '010-fix-python-path-traversal', language: 'Python', category: 'bugfix',
    title: '阻止路径逃逸', description: 'paths.py 的 safe_path(root, user_path) 返回解析后的绝对 Path；任何逃逸根目录的路径（..、绝对路径、相邻前缀目录、指向外部的符号链接）都必须抛出 ValueError。根目录内的合法路径应正常返回。',
    files: {
      'paths.py': 'from pathlib import Path\n\ndef safe_path(root, user_path):\n    return Path(root) / user_path\n',
      'tests/test_paths.py': `import os
import tempfile
import unittest
from pathlib import Path
from paths import safe_path
class PathsTest(unittest.TestCase):
    def test_containment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "root"
            root.mkdir()
            self.assertEqual(safe_path(root, "a/../b.txt"), (root / "b.txt").resolve())
            for user_path in ("../secret.txt", "../root-other/a", str(Path(directory) / "outside")):
                with self.assertRaises(ValueError): safe_path(root, user_path)
            try:
                os.symlink(directory, root / "escape", target_is_directory=True)
            except OSError:
                return
            with self.assertRaises(ValueError): safe_path(root, "escape/outside")
`,
    },
    solution: { 'paths.py': 'from pathlib import Path\n\ndef safe_path(root, user_path):\n    root = Path(root).resolve()\n    target = (root / user_path).resolve()\n    target.relative_to(root)\n    return target\n' },
  },
];
