export async function previewInvoice(db: Store, id: string) {
  const invoice = await db.read(id);
  await db.save(id, { ...invoice, sent: true });
  return invoice;
}
