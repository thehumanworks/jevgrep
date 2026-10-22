export async function saveUser(db: Store, user: User) {
  try {
    await db.save(user);
  } catch (error) {
    return { ok: false, error };
  }
  return { ok: true };
}
