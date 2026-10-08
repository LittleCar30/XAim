import unittest
from user import valid_user
from order import valid_order
class ValidatorTest(unittest.TestCase):
    def test_behavior(self):
        for value in (None, "", "  "): self.assertFalse(valid_user(value)); self.assertFalse(valid_order(value))
        for value in ("alice", "A-1"): self.assertTrue(valid_user(value)); self.assertTrue(valid_order(value))
