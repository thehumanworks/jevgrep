def save_user(store, user):
    try:
        store.save(user)
    except IOError as error:
        return {"ok": False, "error": str(error)}
    return {"ok": True}
