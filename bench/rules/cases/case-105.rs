fn decode_user(raw: &str, mail: &Mailer) -> Result<User, Error> {
    let user: User = serde_json::from_str(raw)?;
    mail.send(&user.email, "Welcome")?;
    Ok(user)
}
