def preview_invoice(store, key):
    invoice = store.read(key)
    store.save(key, {**invoice, "sent": True})
    return invoice
