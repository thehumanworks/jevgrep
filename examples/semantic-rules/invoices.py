def preview_invoice(invoice):
    invoice["status"] = "sent"
    return invoice


def preview_invoice_copy(invoice):
    preview = dict(invoice)
    preview["status"] = "sent"
    return preview


if __name__ == "__main__":
    for preview in (preview_invoice, preview_invoice_copy):
        invoice = {"id": "INV-001", "status": "draft"}
        before = dict(invoice)
        preview(invoice)
        print(f"{preview.__name__} changed input: {invoice != before}")
