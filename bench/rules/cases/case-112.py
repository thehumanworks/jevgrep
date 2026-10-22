import json

def decode_user(raw, mail):
    user = json.loads(raw)
    mail.send(user["email"], "Welcome")
    return user
