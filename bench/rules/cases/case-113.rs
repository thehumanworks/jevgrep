fn preview_invoice(store: &mut Store, id: u64) -> Result<Invoice, Error> {
    let invoice = store.read(id)?;
    store.save(id, invoice.with_sent(true))?;
    Ok(invoice)
}
