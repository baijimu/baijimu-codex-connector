"""Publish a connector through the source owner and verify immutable bytes."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit


def require(condition, message):
    if not condition:
        raise ValueError(message)


def checksum(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return "sha256:" + digest.hexdigest()


class Cli:
    def __init__(self, executable, workspace):
        self.executable, self.workspace = executable, workspace

    def call(self, *args, missing=False):
        result = subprocess.run([self.executable, *args, "--workspace-id", str(self.workspace), "--json"],
                                capture_output=True, text=True)
        if result.returncode:
            # Only the exact source-owner absence response permits creation.
            if missing and re.fullmatch(
                r"Error: downstream returned HTTP 200 OK with error code LOCAL_APP_SERVICE_NOT_FOUND\s*",
                result.stderr,
            ):
                return None
            sys.stderr.write(result.stderr)
            raise RuntimeError("Baijimu CLI operation failed")
        envelope = json.loads(result.stdout)
        require(envelope.get("errorCode") == "0" and envelope.get("contractVersion") == "1.0.0",
                "Invalid source response envelope")
        return envelope["data"]

    def api(self, path, **query):
        return self.call("api", "get", "/local-app-service/api/local-app-market/" + path,
                         "--query", json.dumps({"workspaceId": self.workspace, **query}))


def validate_release(version, connector, oss):
    require(re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", version),
            "Invalid stable release version")
    require(connector["schemaVersion"] == "3.0.0" and connector["version"] == version
            and connector["source"]["type"] == "github"
            and connector["source"]["revision"] == "v" + version, "Connector release identity mismatch")
    require(oss["schemaVersion"] == "2.0.0" and oss["appId"] == connector["appId"]
            and oss["version"] == version and oss["releaseTag"] == "v" + version,
            "OSS release identity mismatch")
    artifacts = oss["artifacts"]
    require(artifacts and len({(a["platform"], a["arch"]) for a in artifacts}) == len(artifacts),
            "Missing or duplicate platform artifacts")
    for artifact in artifacts:
        url = urlsplit(artifact["source"])
        require(url.scheme == "https" and url.hostname and not url.username and not url.password,
                "Invalid public artifact URL")
        require(re.fullmatch(r"sha256:[0-9a-f]{64}", artifact["checksum"]), "Invalid artifact checksum")
        require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,190}", url.path.rsplit("/", 1)[-1]),
                "Invalid artifact file name")
    return {**connector, "applicationType": "connector", "artifacts": artifacts}


def verify_frozen(frozen, source, manifest, originals):
    require(frozen["contractVersion"] == "1.0.0" and frozen["source"] == source,
            "Frozen source identity mismatch")
    content = frozen["content"]
    require(content["applicationType"] == "connector" and content["manifest"] == manifest
            and content["sourceRevision"] == "v" + source["version"], "Frozen content mismatch")
    rows = {(r["platform"], r["architecture"]): r for r in content["artifacts"]}
    require(len(rows) == len(originals) == len(content["artifacts"]), "Frozen artifact targets mismatch")
    for artifact, path in originals:
        row = rows[(artifact["platform"], artifact["arch"])]
        require(row["fileName"] == urlsplit(artifact["source"]).path.rsplit("/", 1)[-1]
                and row["sizeBytes"] == path.stat().st_size, "Frozen artifact metadata mismatch")
    return rows


def publish(cli, version, connector, oss, directory):
    manifest = validate_release(version, connector, oss)
    app_id = connector["appId"]
    app = cli.call("local-app", "get", app_id, missing=True)
    if app is None:
        cli.call("local-app", "create", app_id, "--application-type", "connector",
                 "--name", connector["name"], "--description", connector.get("description", ""))
        app = cli.call("local-app", "get", app_id)
    require(app["source"]["appId"] == app_id and app["workspaceId"] == cli.workspace
            and app["applicationType"] == "connector" and app["registered"] is True,
            "Source application ownership mismatch")
    source = {"application": app["source"], "version": version}
    originals = []
    for index, artifact in enumerate(oss["artifacts"]):
        path = directory / f"original-{index}.zip"
        subprocess.run(["curl", "--disable", "--fail", "--silent", "--show-error", "--location",
                        "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "15",
                        "--max-time", "900", "--output", str(path), artifact["source"]], check=True)
        require(checksum(path) == artifact["checksum"], "Published artifact checksum mismatch")
        originals.append((artifact, path))
    frozen = cli.call("local-app", "version", "get", app_id, version, missing=True)
    if frozen is None:
        uploaded = []
        for artifact, path in originals:
            name = urlsplit(artifact["source"]).path.rsplit("/", 1)[-1]
            row = cli.call("local-app", "artifact", "upload", app_id, "--file", str(path), "--file-name", name)
            require(row["fileName"] == name and row["sizeBytes"] == path.stat().st_size,
                    "Uploaded artifact metadata mismatch")
            uploaded.append({"artifactId": row["artifactId"], "fileName": name, "sizeBytes": row["sizeBytes"],
                             "platform": artifact["platform"], "architecture": artifact["arch"]})
        content = {"applicationType": "connector", "manifest": manifest,
                   "sourceRevision": "v" + version, "artifacts": uploaded}
        request = directory / "version.json"
        request.write_text(json.dumps(content, ensure_ascii=False))
        cli.call("local-app", "version", "freeze", app_id, version, "--data", "@" + str(request))
        frozen = cli.call("local-app", "version", "get", app_id, version)
    rows = verify_frozen(frozen, source, manifest, originals)
    for index, (artifact, original) in enumerate(originals):
        row = rows[(artifact["platform"], artifact["arch"])]
        path = directory / f"source-{index}.zip"
        cli.call("local-app", "artifact", "download", app_id, version, row["artifactId"], "--output", str(path))
        require(checksum(path) == checksum(original), "Source stored artifact checksum mismatch")
    receipt = cli.call("local-app", "publication", "get", app_id, version, missing=True)
    if receipt is not None:
        require(receipt["source"] == source, "Publication source mismatch")
    if receipt is None or receipt["state"] == "RECEIVING":
        cli.call("local-app", "submit", app_id, version)
    for _ in range(20):
        receipt = cli.call("local-app", "publication", "get", app_id, version)
        require(receipt["source"] == source, "Publication source mismatch")
        state = receipt["state"]
        if state in ("PENDING_REVIEW", "PUBLISHED"):
            if state == "PUBLISHED":
                verify_market(cli, frozen)
            return state
        require(state == "RECEIVING", "Publication requires owner review: " + state)
        time.sleep(3)
    raise RuntimeError("Publication receipt remains RECEIVING")


def verify_market(cli, frozen):
    market = cli.api("context")
    cursor, seen, matches = None, set(), []
    while True:
        page = cli.api("listings", **({"after": cursor} if cursor else {}))
        for row in page["items"]:
            require(row["marketKey"] == market, "Consumer market authority mismatch")
            if row["frozenVersion"]["source"]["application"] == frozen["source"]["application"]:
                matches.append(row["listingId"])
        cursor = page["nextCursor"]
        if cursor is None:
            break
        require(cursor not in seen, "Market pagination cycle")
        seen.add(cursor)
    require(len(matches) == 1, "Published market listing is not unique or visible")
    row = cli.api(f"listings/{matches[0]}/versions/{frozen['source']['version']}")
    require(row["marketKey"] == market and row["listingId"] == matches[0]
            and row["frozenVersion"] == frozen, "Published market content mismatch")


def main():
    version, connector_path, oss_path = sys.argv[1:]
    workspace = int(os.environ["LOCAL_APP_OWNER_WORKSPACE_ID"])
    require(workspace > 0, "Invalid owner workspace")
    with tempfile.TemporaryDirectory(prefix="codex-source-publication-") as temp:
        directory = Path(temp)
        os.environ["BAIJIMU_AUTH_FILE"] = str(directory / "auth.json")
        executable = os.environ["BAIJIMU_CLI"]
        subprocess.run([executable, "auth", "login", "--token", os.environ["LOCAL_APP_MARKET_PUBLISH_TOKEN"],
                        "--workspace-id", str(workspace), "--no-browser", "--json"],
                       stdout=subprocess.DEVNULL, check=True)
        state = publish(Cli(executable, workspace), version, json.loads(Path(connector_path).read_text()),
                        json.loads(Path(oss_path).read_text()), directory)
        Path(os.environ["MARKET_PUBLICATION_STATUS_FILE"]).write_text(state + "\n")
        print("source publication state=" + state)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        # A failed auth command contains the PAT in argv. Never stringify it.
        print(f"publication subprocess failed (exit {error.returncode})", file=sys.stderr)
        sys.exit(1)
    except (ValueError, RuntimeError, KeyError, OSError) as error:
        print("publication validation failed: " + str(error), file=sys.stderr)
        sys.exit(1)
