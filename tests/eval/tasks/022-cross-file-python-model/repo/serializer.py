class User:
    def __init__(self, name): self.name = name
def serialize(user): return {"name": user.name}
