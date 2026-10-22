import json

def decode_user(raw):
    user = json.loads(raw)
    if not isinstance(user["email"], str):
        raise ValueError("Invalid email")
    return user
