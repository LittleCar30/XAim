import { slug } from "slug-kit";
export function title(value: string): string { return slug(value); }
