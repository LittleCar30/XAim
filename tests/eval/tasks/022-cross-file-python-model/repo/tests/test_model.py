import unittest
from service import User, greet
from serializer import serialize
class ModelTest(unittest.TestCase):
    def test_model(self): self.assertEqual(greet(User("A")), "Hello A"); self.assertEqual(serialize(User("A")), {"name": "A"})
