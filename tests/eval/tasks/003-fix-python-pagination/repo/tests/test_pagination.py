import unittest
from pagination import pages
class PaginationTest(unittest.TestCase):
    def test_tail(self):
        self.assertEqual(pages([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]])
        self.assertEqual(pages([1], 3), [[1]])
    def test_empty_exact_invalid(self):
        self.assertEqual(pages([], 2), [])
        self.assertEqual(pages([1, 2], 2), [[1, 2]])
        for size in (0, -1):
            with self.assertRaises(ValueError): pages([1], size)
