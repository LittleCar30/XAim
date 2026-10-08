import unittest
from cache import add
class CacheTest(unittest.TestCase):
    def test_isolation(self):
        first = add("a")
        second = add("b")
        self.assertEqual(first, ["a"])
        self.assertEqual(second, ["b"])
        self.assertIsNot(first, second)
    def test_explicit_bucket(self):
        bucket = []
        self.assertIs(add("c", bucket), bucket)
        self.assertEqual(bucket, ["c"])
