def valid_order(reference):
    return isinstance(reference, str) and bool(reference.strip())
