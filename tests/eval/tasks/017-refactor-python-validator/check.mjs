import assert from 'node:assert/strict';
export function check({ trusted }) {
  trusted(['python','-c', 'import validators, user, order; validators.require_text = lambda value: "sentinel"; user.require_text = validators.require_text; order.require_text = validators.require_text; assert user.valid_user(None) == "sentinel"; assert order.valid_order(None) == "sentinel"'], {});
  trusted(['python','-c', 'import ast; from pathlib import Path; assert all(any(isinstance(n, ast.ImportFrom) and n.module == "validators" for n in ast.walk(ast.parse(Path(p).read_text()))) for p in ["user.py","order.py"])'], {});
}
