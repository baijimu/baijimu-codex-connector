import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("publisher", Path(__file__).resolve().parents[1] / "tools/release/publish-market.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class Consumer:
    def __init__(self):
        self.frozen = {"source": {"application": {"environmentKey": "author-a", "appId": "example"}, "version": "1.0.0"}}
        self.row = {"contractVersion": "3.0.0", "listingId": "catalog-a", "frozenVersion": copy.deepcopy(self.frozen)}
        self.exact = copy.deepcopy(self.row)
        self.calls = []

    def api(self, path, **query):
        self.calls.append(path)
        if path == "listings":
            return {"items": [self.row], "nextCursor": None}
        if path == "listings/catalog-a/versions/1.0.0":
            return self.exact
        raise AssertionError("Unexpected read: " + path)


class MarketIdentityTests(unittest.TestCase):
    def test_resolves_by_source_without_market_context(self):
        cli = Consumer()
        publisher.verify_market(cli, cli.frozen)
        self.assertEqual(len(cli.calls), 2)

    def test_same_named_foreign_source_is_not_selected(self):
        cli = Consumer()
        cli.row["frozenVersion"]["source"]["application"]["environmentKey"] = "author-b"
        with self.assertRaises((ValueError, RuntimeError)):
            publisher.verify_market(cli, cli.frozen)

    def test_exact_version_and_protocol_are_checked(self):
        for changed in ["protocol", "version"]:
            cli = Consumer()
            if changed == "protocol":
                cli.exact["contractVersion"] = "2.0.0"
            else:
                cli.exact["frozenVersion"]["source"]["version"] = "1.0.1"
            with self.assertRaises((ValueError, RuntimeError)):
                publisher.verify_market(cli, cli.frozen)
