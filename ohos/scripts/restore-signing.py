"""Restore the release owner's tar.gz signing bundle in a disposable CI checkout."""

import base64
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import sys
import tarfile
import tempfile

CONFIG = "signing.local.json"


class SigningError(Exception):
    pass


def signing_path(name):
    path = PurePosixPath(name)
    return not path.is_absolute() and ".." not in path.parts and (
        name == CONFIG or path.parts[:1] == ("signing",)
    )


def signing_filter(member, destination):
    if not signing_path(member.name) or not (member.isfile() or member.isdir()):
        raise SigningError("Bundle may only contain signing.local.json and regular files under signing/.")
    member = tarfile.data_filter(member, destination)
    return member.replace(mode=0o700 if member.isdir() else 0o600)


def validate_config(directory):
    config = json.loads((directory / CONFIG).read_text())
    material = next(item["material"] for item in config["signingConfigs"]
                    if item["name"] == "default" and item["type"] == "HarmonyOS")
    for key in ("storeFile", "profile", "certpath"):
        name = material[key]
        if not name.startswith("signing/") or not signing_path(name) or not (directory / name).is_file():
            raise SigningError("Certificate, keystore and profile must use existing relative signing/ paths.")
    material_dir = (directory / material["storeFile"]).parent / "material"
    if not material_dir.is_dir() or not any(material_dir.iterdir()):
        raise SigningError("Keep the matching DevEco material directory beside the keystore.")
    passwords = [material[key] for key in ("storePassword", "keyPassword")]
    if any(not re.fullmatch(r"[0-9a-fA-F]{32,}", value) or len(value) % 2 for value in passwords):
        raise SigningError("Use DevEco's encrypted storePassword and keyPassword values.")
    return passwords


def restore():
    if os.environ.get("CI") != "true":
        raise SigningError("Signing restore requires CI=true.")
    app = Path(__file__).resolve().parents[1] / "app"
    if any(os.path.lexists(app / name) for name in (CONFIG, "signing")):
        raise SigningError("Signing inputs already exist; refusing to overwrite them.")
    encoded = os.environ.get("OHOS_SIGNING_BUNDLE_BASE64", "")
    if not encoded:
        raise SigningError("OHOS_SIGNING_BUNDLE_BASE64 is required.")
    archive = base64.b64decode(encoded.strip(), validate=True)
    # Validate in a private temporary directory before changing build inputs.
    with tempfile.TemporaryDirectory(prefix=".ci-signing-", dir=app) as temporary:
        staged = Path(temporary)
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as bundle:
            if sum(member.size for member in bundle.getmembers()) > 8 * 1024 * 1024:
                raise SigningError("Signing bundle exceeds 8 MiB unpacked.")
            bundle.extractall(staged, filter=signing_filter)
        passwords = validate_config(staged)
        if os.environ.get("GITHUB_ACTIONS") == "true":
            for password in set(passwords):
                print(f"::add-mask::{password}")
        (staged / "signing").rename(app / "signing")
        try:
            (staged / CONFIG).rename(app / CONFIG)
        except OSError:
            shutil.rmtree(app / "signing")
            raise
    print("Restored CI signing inputs.")


if __name__ == "__main__":
    try:
        restore()
    except SigningError as error:
        sys.exit(str(error))
    except Exception:
        # Archive/JSON/OS errors may contain credential fragments or paths.
        sys.exit("Cannot restore signing inputs; check the archive, configuration and file permissions.")
