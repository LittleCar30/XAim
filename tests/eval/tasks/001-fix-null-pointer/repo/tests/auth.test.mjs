import assert from "node:assert/strict";
import test from "node:test";
import { login } from "../dist/auth.js";
test("valid", () => assert.equal(login({name: "Alice"}), "Alice"));
test("empty name", () => assert.equal(login({name: ""}), ""));
test("null", () => assert.equal(login(null), null));
test("undefined", () => assert.equal(login(undefined), null));
