export async function fetchData(url: string) {
  try { return await fetch(url); } catch { return null; }
}
