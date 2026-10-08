from pathlib import Path

def safe_path(root, user_path):
    return Path(root) / user_path
