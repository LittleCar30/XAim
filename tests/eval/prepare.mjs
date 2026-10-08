import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { run } from './verify.mjs';

const taskDir = path.resolve(process.argv[2]);
const repoDir = path.resolve(process.argv[3]);
const metadata = path.join(taskDir, 'task.json');
if (existsSync(metadata)) {
  const task = JSON.parse(readFileSync(metadata, 'utf8'));
  for (const command of task.prepare || []) {
    const result = run(command, repoDir);
    process.stdout.write(`$ ${command.join(' ')}\n${result.output}\n`);
    if (result.status !== 0) process.exit(result.status || 1);
  }
}
