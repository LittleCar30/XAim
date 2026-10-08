export function average(values: number[]): number {
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}
