fn preview_invoice(store: &Store, id: u64) -> Result<Invoice, Error> {
    let invoice = store.read(id)?;
    let sent = invoice.with_sent(true); // returns a new value, no I/O
    Ok(sent)
}
