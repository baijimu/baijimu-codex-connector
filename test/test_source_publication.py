import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("publisher", ROOT / "tools/release/publish-market.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class SourcePublicationTest(unittest.TestCase):
    def setUp(self):
        self.connector = json.loads((ROOT / "connector.json").read_text())
        self.version = self.connector["version"]
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.original = self.directory / "artifact.zip"
        self.original.write_bytes(b"immutable connector artifact")
        self.artifact = {"platform": "windows", "arch": "x86_64", "source": "https://example.test/artifact.zip",
                         "checksum": publisher.checksum(self.original)}
        self.oss = {"schemaVersion": "2.0.0", "appId": self.connector["appId"], "version": self.version,
                    "releaseTag": "v" + self.version, "artifacts": [self.artifact]}
        self.source = {"application": {"appId": self.connector["appId"], "environmentKey": "test"},
                       "version": self.version}

    def frozen(self):
        return {"contractVersion": "1.0.0", "source": self.source, "content": {
            "applicationType": "connector", "sourceRevision": "v" + self.version,
            "manifest": publisher.validate_release(self.version, self.connector, self.oss),
            "artifacts": [{"platform": "windows", "architecture": "x86_64", "fileName": "artifact.zip",
                           "sizeBytes": self.original.stat().st_size, "artifactId": "artifact-id"}]}}

    def test_preserves_complete_connector_manifest_and_rejects_identity_drift(self):
        manifest = publisher.validate_release(self.version, self.connector, self.oss)
        self.assertEqual(manifest, self.connector)
        self.assertNotIn("applicationType", manifest)
        self.assertNotIn("artifacts", manifest)
        for field in ("runtime", "methods", "events", "management", "source"):
            self.assertEqual(manifest[field], self.connector[field])
        wrong = copy.deepcopy(self.oss)
        wrong["appId"] += "-other"
        with self.assertRaises(ValueError):
            publisher.validate_release(self.version, self.connector, wrong)

    def test_existing_version_must_match_manifest_and_artifact_bytes_metadata(self):
        manifest = publisher.validate_release(self.version, self.connector, self.oss)
        frozen = self.frozen()
        publisher.verify_frozen(frozen, self.source, manifest, [(self.artifact, self.original)])
        for field, value in [("sourceRevision", "different"), ("manifest", {})]:
            changed = copy.deepcopy(frozen)
            changed["content"][field] = value
            with self.assertRaises(ValueError):
                publisher.verify_frozen(changed, self.source, manifest, [(self.artifact, self.original)])
        frozen["content"]["artifacts"][0]["sizeBytes"] += 1
        with self.assertRaises(ValueError):
            publisher.verify_frozen(frozen, self.source, manifest, [(self.artifact, self.original)])

    def test_market_queries_use_the_public_cli_name_value_contract(self):
        cli = publisher.Cli("baijimu", 17)
        with patch.object(cli, "call", return_value={}) as call:
            cli.api("listings", after="cursor-id")
        call.assert_called_once_with("api", "get", "/local-app-service/api/local-app-market/listings",
                                     "--query", "workspaceId=17", "--query", "after=cursor-id")

    def test_only_exact_source_absence_is_creatable(self):
        cli = publisher.Cli("baijimu", 1)
        for stderr, missing in [
            ("Error: downstream returned HTTP 200 OK with error code LOCAL_APP_SERVICE_NOT_FOUND\n", True),
            ("Error: downstream returned HTTP 200 OK with error code LOCAL_APP_SERVICE_FORBIDDEN\n", False),
            ("Error: HTTP 404 Not Found\n", False),
        ]:
            result = type("Result", (), {"returncode": 1, "stderr": stderr})()
            with patch.object(publisher.subprocess, "run", return_value=result):
                if missing:
                    self.assertIsNone(cli.call("local-app", "get", "fixture", missing=True))
                else:
                    with self.assertRaises(RuntimeError):
                        cli.call("local-app", "get", "fixture", missing=True)

    def test_pending_review_is_not_approved_or_recreated_on_rerun(self):
        parent, calls = self, []
        class FakeCli:
            workspace = 1
            def call(self, *args, **kwargs):
                calls.append(args)
                if args[:2] == ("local-app", "get"):
                    return {"source": parent.source["application"], "workspaceId": 1,
                            "applicationType": "connector", "registered": True}
                if args[:3] == ("local-app", "version", "get"):
                    return parent.frozen()
                if args[:3] == ("local-app", "artifact", "download"):
                    Path(args[-1]).write_bytes(parent.original.read_bytes())
                    return {}
                if args[:3] == ("local-app", "publication", "get"):
                    return {"source": parent.source, "state": "PENDING_REVIEW"}
                raise AssertionError(args)
        def download(args, **kwargs):
            Path(args[args.index("--output") + 1]).write_bytes(self.original.read_bytes())
        with patch.object(publisher.subprocess, "run", side_effect=download):
            state = publisher.publish(FakeCli(), self.version, self.connector, self.oss, self.directory)
        self.assertEqual(state, "PENDING_REVIEW")
        self.assertFalse(any("freeze" in args or "submit" in args or "create" in args for args in calls))

    def test_new_source_freezes_uploaded_artifacts_and_submits_once(self):
        parent, calls = self, []
        class FakeCli:
            workspace = 1
            app_exists, frozen, submitted = False, None, False
            def call(self, *args, **kwargs):
                calls.append(args)
                if args[:2] == ("local-app", "get"):
                    return ({"source": parent.source["application"], "workspaceId": 1,
                             "applicationType": "connector", "registered": True} if self.app_exists else None)
                if args[:2] == ("local-app", "create"):
                    self.app_exists = True
                    return {}
                if args[:3] == ("local-app", "version", "get"):
                    return self.frozen
                if args[:3] == ("local-app", "artifact", "upload"):
                    return {"artifactId": "uploaded-id", "fileName": "artifact.zip",
                            "sizeBytes": parent.original.stat().st_size}
                if args[:3] == ("local-app", "version", "freeze"):
                    self.frozen = {"contractVersion": "1.0.0", "source": parent.source,
                                   "content": json.loads(Path(args[-1][1:]).read_text())}
                    return self.frozen
                if args[:3] == ("local-app", "artifact", "download"):
                    parent.assertEqual(args[-3], "uploaded-id")
                    Path(args[-1]).write_bytes(parent.original.read_bytes())
                    return {}
                if args[:3] == ("local-app", "publication", "get"):
                    return {"source": parent.source, "state": "PENDING_REVIEW"} if self.submitted else None
                if args[:2] == ("local-app", "submit"):
                    self.submitted = True
                    return {}
                raise AssertionError(args)
        def download(args, **kwargs):
            Path(args[args.index("--output") + 1]).write_bytes(self.original.read_bytes())
        with patch.object(publisher.subprocess, "run", side_effect=download):
            state = publisher.publish(FakeCli(), self.version, self.connector, self.oss, self.directory)
        self.assertEqual(state, "PENDING_REVIEW")
        self.assertEqual(sum(args[:2] == ("local-app", "submit") for args in calls), 1)

    def test_rejects_obsolete_distribution_fields_inside_connector_manifest(self):
        for field, value in [("applicationType", "connector"), ("artifacts", [])]:
            invalid = {**self.connector, field: value}
            with self.assertRaises(ValueError):
                publisher.validate_release(self.version, invalid, self.oss)

    def test_duplicate_platform_targets_are_rejected(self):
        self.oss["artifacts"].append(dict(self.artifact))
        with self.assertRaises(ValueError):
            publisher.validate_release(self.version, self.connector, self.oss)


if __name__ == "__main__":
    unittest.main()
