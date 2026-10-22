export async function previewInvoice(db: Store, id: string) {
  const invoice = await db.read(id);
  const sent = { ...invoice, sent: true };
  return sent;
}
