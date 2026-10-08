import sys
sys.path.insert(0, ".venv/lib")
import unittest
from app import accept
class AppTest(unittest.TestCase):
    def test_values(self): self.assertTrue(accept(2)); self.assertFalse(accept(0)); self.assertFalse(accept(-1))
