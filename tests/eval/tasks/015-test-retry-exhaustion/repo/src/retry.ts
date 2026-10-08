export function retry(operation: () => boolean, attempts: number): boolean {
  for (let i = 0; i < attempts; i++) { if (operation()) return true; }
  return false;
}
