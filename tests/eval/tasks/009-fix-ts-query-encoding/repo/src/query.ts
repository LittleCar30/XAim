export function query(values: Record<string, string>): string {
  return Object.entries(values).map(([key, value]) => `${key}=${value}`).join("&");
}
