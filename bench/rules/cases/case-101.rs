fn decode_user(raw: &str) -> Result<User, Error> {
    let user: User = serde_json::from_str(raw)?;
    if user.email.is_empty() { return Err(Error::InvalidEmail); }
    Ok(user)
}
