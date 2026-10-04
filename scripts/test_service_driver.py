import importlib.util
import pathlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('driver', pathlib.Path(__file__).with_name('service-driver.py'))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)

class Restart(unittest.TestCase):
    def test_unknown_broadcast_is_not_retried(self):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / 'journal.json'
            driver.save(p, {'intent': {'Deliver': {'id': 1}}})
            with self.assertRaisesRegex(RuntimeError, 'ambiguous'):
                driver.reconcile(p, lambda *a: self.fail('must not query/broadcast unknown hash'))
    def test_confirmed_failed_transaction_allows_retry(self):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / 'journal.json'
            driver.save(p, {'txhash': 'ABC'})
            driver.reconcile(p, lambda *a: {'code': 5, 'height': '12'})
            self.assertFalse(p.exists())
    def test_unindexed_transaction_stays_pending(self):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / 'journal.json'
            driver.save(p, {'txhash': 'ABC'})
            def unavailable(*a): raise RuntimeError('not indexed')
            with self.assertRaises(RuntimeError): driver.reconcile(p, unavailable)
            self.assertTrue(p.exists())

if __name__ == '__main__': unittest.main()
