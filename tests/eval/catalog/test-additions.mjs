export default [
  {
    id: '011-test-discount-boundaries', language: 'TS', category: 'tests', title: '补充折扣边界测试',
    description: '仅修改 tests/discount.test.mjs。覆盖折扣率 0、1、非法负数和大于 1 的数值；非法折扣率应抛 RangeError。新增测试必须能发现不拒绝非法折扣率的变异实现。',
    files: {
      'src/discount.ts': 'export function discount(price: number, rate: number): number {\n  if (rate < 0 || rate > 1) throw new RangeError("rate");\n  return price * (1 - rate);\n}\n',
      'tests/discount.test.mjs': `import assert from 'node:assert/strict';
import test from 'node:test';
import { discount } from '../dist/discount.js';
test('ordinary discount', () => assert.equal(discount(100, 0.2), 80));
`,
    },
    solution: { 'tests/discount.test.mjs': `import assert from 'node:assert/strict';
import test from 'node:test';
import { discount } from '../dist/discount.js';
test('ordinary discount', () => assert.equal(discount(100, 0.2), 80));
test('boundaries', () => { assert.equal(discount(100, 0), 100); assert.equal(discount(100, 1), 0); });
test('invalid rate', () => { assert.throws(() => discount(100, -0.1), RangeError); assert.throws(() => discount(100, 1.1), RangeError); });
` },
    mutations: [
      { name: 'negative rate accepted', file: 'src/discount.ts', before: 'rate < 0 || rate > 1', after: 'rate > 1', command: ['npm', 'test'], failure: 'AssertionError' },
      { name: 'excessive rate accepted', file: 'src/discount.ts', before: 'rate < 0 || rate > 1', after: 'rate < 0', command: ['npm', 'test'], failure: 'AssertionError' },
      { name: 'zero discount wrong', file: 'src/discount.ts', before: 'return price * (1 - rate);', after: 'return rate === 0 ? 0 : price * (1 - rate);', command: ['npm', 'test'], failure: 'AssertionError' },
      { name: 'full discount wrong', file: 'src/discount.ts', before: 'return price * (1 - rate);', after: 'return rate === 1 ? price : price * (1 - rate);', command: ['npm', 'test'], failure: 'AssertionError' },
    ],
  },
  {
    id: '012-test-python-unicode', language: 'Python', category: 'tests', title: '补充 Unicode slug 测试',
    description: '仅修改 tests/test_slug.py。覆盖中文、连续空白、空字符串，保留 Unicode 并以单个连字符分隔。测试必须发现 ASCII 过滤和空白拆分错误。',
    files: {
      'slug.py': 'def slug(text):\n    return "-".join(text.lower().split())\n',
      'tests/test_slug.py': `import unittest
from slug import slug
class SlugTest(unittest.TestCase):
    def test_ascii(self):
        self.assertEqual(slug("Hello World"), "hello-world")
`,
    },
    solution: { 'tests/test_slug.py': `import unittest
from slug import slug
class SlugTest(unittest.TestCase):
    def test_ascii(self):
        self.assertEqual(slug("Hello World"), "hello-world")
    def test_unicode(self):
        self.assertEqual(slug("你好 世界"), "你好-世界")
    def test_whitespace(self):
        self.assertEqual(slug("  A\\t  B  "), "a-b")
    def test_empty(self):
        self.assertEqual(slug(""), "")
` },
    mutations: [
      { name: 'unicode removed', file: 'slug.py', before: 'text.lower().split()', after: 'text.encode("ascii", "ignore").decode().lower().split()', command: ['python', '-m', 'unittest', 'discover', '-s', 'tests'], failure: 'AssertionError' },
      { name: 'whitespace not collapsed', file: 'slug.py', before: 'text.lower().split()', after: 'text.lower().split(" ")', command: ['python', '-m', 'unittest', 'discover', '-s', 'tests'], failure: 'AssertionError' },
    ],
  },
  {
    id: '013-test-rust-empty-range', language: 'Rust', category: 'tests', title: '补充 Rust 范围边界测试',
    description: '仅修改 tests/range.rs。覆盖相等端点、反向范围，均应返回空 Vec。测试必须发现两种边界变异，不能改生产代码。',
    files: {
      'src/lib.rs': 'pub fn range(start: i32, end: i32) -> Vec<i32> {\n    if start >= end { return Vec::new(); }\n    (start..end).collect()\n}\n',
      'tests/range.rs': 'use fixture::range;\n#[test] fn ordinary() { assert_eq!(range(1, 3), vec![1, 2]); }\n',
    },
    solution: { 'tests/range.rs': 'use fixture::range;\n#[test] fn ordinary() { assert_eq!(range(1, 3), vec![1, 2]); }\n#[test] fn equal() { assert!(range(3, 3).is_empty()); }\n#[test] fn reversed() { assert!(range(4, 2).is_empty()); }\n' },
    mutations: [
      { name: 'equal endpoints wrong', file: 'src/lib.rs', before: 'if start >= end { return Vec::new(); }', after: 'if start == end { return vec![start]; }\n    if start > end { return Vec::new(); }', command: ['cargo', 'test', '--offline'], failure: 'panicked at' },
      { name: 'reversed endpoints wrong', file: 'src/lib.rs', before: 'if start >= end { return Vec::new(); }', after: 'if start > end { return vec![start]; }\n    if start == end { return Vec::new(); }', command: ['cargo', 'test', '--offline'], failure: 'panicked at' },
    ],
  },
  {
    id: '014-test-go-empty-queue', language: 'Go', category: 'tests', title: '补充 Go 队列测试',
    description: '仅修改 queue_test.go。检查空队列 Pop 返回 (0,false)、FIFO、耗尽后仍返回 (0,false)。测试必须发现空队列和 FIFO 变异。',
    files: {
      'queue.go': `package fixture
type Queue struct { items []int }
func (q *Queue) Push(value int) { q.items = append(q.items, value) }
func (q *Queue) Pop() (int, bool) {
    if len(q.items) == 0 { return 0, false }
    result := q.items[0]
    q.items = q.items[1:]
    return result, true
}
`,
      'queue_test.go': `package fixture
import "testing"
func TestOne(t *testing.T) { q := &Queue{}; q.Push(7); value, ok := q.Pop(); if value != 7 || !ok { t.Fatal("wrong value") } }
`,
    },
    solution: { 'queue_test.go': `package fixture
import "testing"
func TestOne(t *testing.T) { q := &Queue{}; q.Push(7); value, ok := q.Pop(); if value != 7 || !ok { t.Fatal("wrong value") } }
func TestEmpty(t *testing.T) { q := &Queue{}; value, ok := q.Pop(); if value != 0 || ok { t.Fatal("empty queue") } }
func TestFIFO(t *testing.T) {
    q := &Queue{}; q.Push(1); q.Push(2)
    for _, expected := range []int{1, 2} { value, ok := q.Pop(); if !ok || value != expected { t.Fatal("FIFO") } }
    value, ok := q.Pop(); if value != 0 || ok { t.Fatal("exhausted") }
}
` },
    mutations: [
      { name: 'empty queue wrong', file: 'queue.go', before: 'return 0, false', after: 'return 0, true', command: ['go', 'test', './...'], failure: '--- FAIL:' },
      { name: 'FIFO wrong', file: 'queue.go', before: 'result := q.items[0]', after: 'result := q.items[len(q.items)-1]', command: ['go', 'test', './...'], failure: '--- FAIL:' },
    ],
  },
  {
    id: '015-test-retry-exhaustion', language: 'TS', category: 'tests', title: '补充重试次数测试',
    description: '仅修改 tests/retry.test.mjs。检查 attempts=0 不调用操作且返回 false；重试耗尽时调用次数等于 attempts 并返回 false。测试必须发现多调用一次的变异。',
    files: {
      'src/retry.ts': 'export function retry(operation: () => boolean, attempts: number): boolean {\n  for (let i = 0; i < attempts; i++) { if (operation()) return true; }\n  return false;\n}\n',
      'tests/retry.test.mjs': `import assert from 'node:assert/strict';
import test from 'node:test';
import { retry } from '../dist/retry.js';
test('immediate success', () => assert.equal(retry(() => true, 2), true));
`,
    },
    solution: { 'tests/retry.test.mjs': `import assert from 'node:assert/strict';
import test from 'node:test';
import { retry } from '../dist/retry.js';
test('immediate success', () => assert.equal(retry(() => true, 2), true));
test('exhausted', () => { let calls = 0; assert.equal(retry(() => { calls++; return false; }, 2), false); assert.equal(calls, 2); });
test('zero attempts', () => { let calls = 0; assert.equal(retry(() => { calls++; return false; }, 0), false); assert.equal(calls, 0); });
` },
    mutations: [
      { name: 'extra retry', file: 'src/retry.ts', before: 'i < attempts', after: 'i <= attempts', command: ['npm', 'test'], failure: 'AssertionError' },
      { name: 'zero attempts call', file: 'src/retry.ts', before: 'i < attempts', after: 'i < Math.max(attempts, 1)', command: ['npm', 'test'], failure: 'AssertionError' },
    ],
  },
];
