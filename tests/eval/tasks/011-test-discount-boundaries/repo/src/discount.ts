export function discount(price: number, rate: number): number {
  if (rate < 0 || rate > 1) throw new RangeError("rate");
  return price * (1 - rate);
}
