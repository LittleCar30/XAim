def valid_user(name):
    return isinstance(name, str) and bool(name.strip())
