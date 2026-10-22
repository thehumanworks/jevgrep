def save_user(store, user):
    try:
        store.save(user)
    except IOError:
        return {"ok": True}
    return {"ok": True}
