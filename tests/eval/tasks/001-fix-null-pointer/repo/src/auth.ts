export interface User {
  name: string;
}

export function login(user: User | null | undefined): string | null {
  return (user as User).name;
}
