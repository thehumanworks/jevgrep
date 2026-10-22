fn save_user(store: &mut Store, user: User) -> Result<(), Error> {
    match store.save(user) {
        Err(error) => Err(error),
        Ok(()) => Ok(()),
    }
}
