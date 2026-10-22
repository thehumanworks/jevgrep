export async function decodeUser(raw: string, mail: Mailer) {
  const user = JSON.parse(raw);
  await mail.send(user.email, "Welcome");
  return user;
}
