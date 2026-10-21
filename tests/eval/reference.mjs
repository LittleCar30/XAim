import { run } from './verify.mjs';

export function applyReferencePatch(repo, patch) {
  // A nested directory otherwise inherits the outer worktree and git apply
  // can skip every patch while returning zero.
  const init = run(['git', 'init', '--quiet'], repo);
  if (init.status !== 0) throw new Error(init.output);
  const result = run(['git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=error', patch], repo);
  if (result.status !== 0) throw new Error(result.output);
}
