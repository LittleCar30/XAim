import unittest
from app import app_timeout
from worker import worker_timeout
class SettingsTest(unittest.TestCase):
    def test_defaults(self): self.assertEqual(app_timeout(), 30); self.assertEqual(worker_timeout(), 30)
