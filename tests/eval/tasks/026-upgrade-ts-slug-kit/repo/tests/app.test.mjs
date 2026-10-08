import test from "node:test";
import assert from "node:assert/strict";
import { title } from "../dist/app.js";
test("slug", () => { assert.equal(title(" Hello World "), "hello-world"); assert.equal(title("你好 世界"), "你好-世界"); });
