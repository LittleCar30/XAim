export default [
  {
    id: '021-cross-file-ts-result', language: 'TS', category: 'cross-file', title: '共享 TypeScript Result 类型', description: '新增 src/result.ts 导出 Result<T>，并让 src/reader.ts 与 src/writer.ts 导入并返回该类型；运行时结果保持不变。',
    files: { 'src/reader.ts': 'export function read(value: string): { ok: boolean; value: string } { return { ok: true, value }; }\n', 'src/writer.ts': 'export function write(value: string): { ok: boolean; value: string } { return { ok: true, value }; }\n', 'tests/result.test.mjs': `import assert from 'node:assert/strict';
import test from 'node:test';
import { read } from '../dist/reader.js'; import { write } from '../dist/writer.js';
test('result', () => { assert.deepEqual(read('a'), { ok: true, value: 'a' }); assert.deepEqual(write('b'), { ok: true, value: 'b' }); });
` },
    solution: { 'src/result.ts': 'export type Result<T> = { ok: boolean; value: T };\n', 'src/reader.ts': 'import type { Result } from "./result.js";\nexport function read(value: string): Result<string> { return { ok: true, value }; }\n', 'src/writer.ts': 'import type { Result } from "./result.js";\nexport function write(value: string): Result<string> { return { ok: true, value }; }\n' },
  },
  {
    id: '022-cross-file-python-model', language: 'Python', category: 'cross-file', title: '共享 Python User 模型', description: '新增 models.py 的 User dataclass；service.py 与 serializer.py 必须从 models 导入并接受该类型，序列化输出保持一致。',
    files: { 'service.py': 'class User:\n    def __init__(self, name): self.name = name\ndef greet(user): return f"Hello {user.name}"\n', 'serializer.py': 'class User:\n    def __init__(self, name): self.name = name\ndef serialize(user): return {"name": user.name}\n', 'tests/test_model.py': `import unittest
from service import User, greet
from serializer import serialize
class ModelTest(unittest.TestCase):
    def test_model(self): self.assertEqual(greet(User("A")), "Hello A"); self.assertEqual(serialize(User("A")), {"name": "A"})
` },
    solution: { 'models.py': 'from dataclasses import dataclass\n@dataclass\nclass User:\n    name: str\n', 'service.py': 'from models import User\ndef greet(user: User): return f"Hello {user.name}"\n', 'serializer.py': 'from models import User\ndef serialize(user: User): return {"name": user.name}\n' },
  },
  {
    id: '023-cross-file-rust-error', language: 'Rust', category: 'cross-file', title: '拆分 Rust Error 类型', description: '新增 error.rs 导出 AppError；reader.rs 与 writer.rs 返回 Result<T, AppError>，main/lib 声明模块，行为保持一致。',
    files: { 'src/reader.rs': 'pub fn read(value: &str) -> Result<String, String> { if value.is_empty() { Err("empty".into()) } else { Ok(value.into()) } }\n', 'src/writer.rs': 'pub fn write(value: &str) -> Result<String, String> { if value.is_empty() { Err("empty".into()) } else { Ok(value.into()) } }\n', 'src/lib.rs': 'pub mod reader;\npub mod writer;\n', 'tests/error.rs': '#[test] fn values() { assert!(fixture::reader::read("a").is_ok()); assert!(fixture::writer::write("").is_err()); }\n' },
    solution: { 'src/error.rs': '#[derive(Debug)]\npub enum AppError { Empty }\n', 'src/reader.rs': 'use crate::error::AppError;\npub fn read(value: &str) -> Result<String, AppError> { if value.is_empty() { Err(AppError::Empty) } else { Ok(value.into()) } }\n', 'src/writer.rs': 'use crate::error::AppError;\npub fn write(value: &str) -> Result<String, AppError> { if value.is_empty() { Err(AppError::Empty) } else { Ok(value.into()) } }\n', 'src/lib.rs': 'pub mod error;\npub mod reader;\npub mod writer;\n' },
  },
  {
    id: '024-cross-file-go-config', language: 'Go', category: 'cross-file', title: '拆分 Go 配置模块', description: '新增 config.go 的 Config 结构和 LoadConfig；server.go 从该模块读取端口，默认端口 8080，测试通过环境变量覆盖。',
    files: { 'server.go': 'package fixture\nfunc Port() int { return 8080 }\n', 'server_test.go': `package fixture
import "testing"
func TestPort(t *testing.T) { if Port() != 8080 { t.Fatal("default port") } }
` },
    solution: { 'config.go': 'package fixture\nimport ("os"; "strconv")\ntype Config struct { Port int }\nfunc LoadConfig() Config { value, err := strconv.Atoi(os.Getenv("PORT")); if err != nil || value <= 0 { value = 8080 }; return Config{Port: value} }\n', 'server.go': 'package fixture\nfunc Port() int { return LoadConfig().Port }\n' },
  },
  {
    id: '025-cross-file-ts-client', language: 'TS', category: 'cross-file', title: '跨文件共享 API 客户端', description: '新增 src/api.ts 的 request 函数；client.ts 必须导入并调用它，保留请求参数和返回 Promise 行为。',
    files: { 'src/client.ts': 'export async function getUser(id: string) { return fetch(`/users/${id}`); }\n', 'tests/client.test.mjs': `import assert from 'node:assert/strict';
import test from 'node:test';
import { getUser } from '../dist/client.js';
test('client delegates', async () => { globalThis.fetch = async (url) => new Response(url); const response = await getUser('7'); assert.equal(await response.text(), '/users/7'); });
` },
    solution: { 'src/api.ts': 'export async function request(url: string): Promise<Response> { return fetch(url); }\n', 'src/client.ts': 'import { request } from "./api.js";\nexport async function getUser(id: string) { return request(`/users/${id}`); }\n' },
  },
];
