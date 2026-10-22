export function decodeUser(raw: string) {
  const user = JSON.parse(raw);
  if (typeof user.email !== "string") throw new Error("Invalid email");
  return user;
}
