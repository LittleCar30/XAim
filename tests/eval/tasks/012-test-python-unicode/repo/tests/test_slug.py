import unittest
from slug import slug
class SlugTest(unittest.TestCase):
    def test_ascii(self):
        self.assertEqual(slug("Hello World"), "hello-world")
