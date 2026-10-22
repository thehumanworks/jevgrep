def preview_invoice(store, key):
    invoice = store.read(key)
    sent = {**invoice, "sent": True}
    return sent
