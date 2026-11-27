"""Loopback-only Bitwarden integration fixture; never use with a real account.

The Rust exporter supplies ciphertext for the SDK's public test@bitwarden.com
account. This server relays that ciphertext without decrypting it, simulates
write outcomes, and never proxies requests or logs request bodies/tokens.

Run --self-test for the stdlib-only HTTP/state tests. For actual DLL/UI tests:
  python tests/fixture_server.py --fixture work/synthetic-sync.json --port 0 --peer
The first stdout line is JSON containing the selected loopback port.
"""

from __future__ import annotations

import argparse
import base64
import copy
import datetime as dt
from email.message import Message
from email.parser import BytesParser
import http.client
import ipaddress
import json
import os
from pathlib import Path
from typing import Any, cast
import re
import socket
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit
import uuid

PUBLIC_EMAIL = "test@bitwarden.com"
PUBLIC_USER_ID = "060000fb-0922-4dd3-b170-6e15cb5df8c8"
PINNED_SDK = "56c7617b23501af70221094cdee59e10b2a9d8d2"
PUBLIC_WRAPPED_USER_KEY = (
    "2.Q/2PhzcC7GdeiMHhWguYAQ==|"
    "GpqzVdr0go0ug5cZh1n+uixeBC3oC90CIe0hd/HWA/pTRDZ8ane4fmsEIcuc8eMK"
    "Ut55Y2q/fbNzsYu41YTZzzsJUSeqVjT8/iTQtgnNdpo=|"
    "dwI+uyvZ1h/iZ03VQ+/wrGEFYVewBUUl/syYgjsNMbE="
)
OUT_OF_DATE = "The item cannot be saved because it is out of date."
MAX_BODY_BYTES = 2 * 1024 * 1024
ATTACHMENT_PATH = re.compile(r"/api/ciphers/([0-9a-fA-F-]{36})/attachment/([a-zA-Z0-9_-]+)")
SEND_UPLOAD_PATH = re.compile(r"/api/sends/([0-9a-f-]{36})/file/([a-zA-Z0-9_-]+)")
UTC = dt.timezone.utc


def parse_date(value: object) -> dt.datetime:
    if not isinstance(value, str):
        raise ValueError("A revision date is required.")
    parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("Revision dates must include a time zone.")
    return parsed.astimezone(UTC)


def format_date(value: dt.datetime) -> str:
    return value.astimezone(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def checked_bind(host: str) -> str:
    # Literal addresses avoid DNS resolution or rebinding to a non-loopback host.
    try:
        address = ipaddress.ip_address(host)
    except ValueError as error:
        raise ValueError("The fixture binds only to literal loopback addresses.") from error
    if not address.is_loopback:
        raise ValueError("The fixture refuses a non-loopback bind.")
    return str(address)


def checked_fixture(value: object) -> dict:
    if not isinstance(value, dict) or not isinstance(value.get("profile"), dict):
        raise ValueError("Expected a synthetic sync response with a profile.")
    profile = value["profile"]
    if profile.get("id") != PUBLIC_USER_ID or profile.get("key") != PUBLIC_WRAPPED_USER_KEY:
        raise ValueError("Refusing a fixture that is not the public SDK test account.")
    if profile.get("email", PUBLIC_EMAIL) != PUBLIC_EMAIL:
        raise ValueError("Refusing a non-public fixture email.")
    if not isinstance(value.get("ciphers"), list) or not isinstance(value.get("folders"), list):
        raise ValueError("The synthetic sync fixture must contain ciphers and folders arrays.")
    ids = set()
    for cipher in value["ciphers"]:
        if not isinstance(cipher, dict):
            raise ValueError("Invalid synthetic cipher record.")
        try:
            key = str(uuid.UUID(cipher.get("id", "")))
        except (ValueError, TypeError, AttributeError) as error:
            raise ValueError("A synthetic cipher has no valid id.") from error
        if key in ids:
            raise ValueError("The synthetic fixture has duplicate cipher ids.")
        ids.add(key)
    folder_ids = set()
    for folder in value["folders"]:
        if not isinstance(folder, dict):
            raise ValueError("Invalid synthetic folder record.")
        try:
            key = str(uuid.UUID(folder.get("id", "")))
        except (ValueError, TypeError, AttributeError) as error:
            raise ValueError("A synthetic folder has no valid id.") from error
        if key in folder_ids:
            raise ValueError("The synthetic fixture has duplicate folder ids.")
        folder_ids.add(key)
    result = copy.deepcopy(value)
    result["profile"]["email"] = PUBLIC_EMAIL
    # Premium is synthetic and local only, so the archive lifecycle can be exercised.
    result["profile"]["premium"] = True
    return result


def load_public_private_key(checkout: str | None) -> str:
    relative = Path("crates/bitwarden-core/src/client/test_accounts.rs")
    if checkout:
        candidate = Path(checkout).expanduser().resolve()
        candidates = [candidate / relative]
    else:
        cargo = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))).expanduser()
        candidates = sorted((cargo / "git/checkouts").glob(
            f"sdk-internal-*/{PINNED_SDK[:7]}*/{relative.as_posix()}"
        ))
    for candidate in candidates:
        if not candidate.is_file():
            continue
        source = candidate.read_text(encoding="utf-8")
        begin = source.find("pub fn test_bitwarden_com_account()")
        end = source.find("pub fn test_bitwarden_com_account_v2()", begin + 1)
        if begin < 0 or end < 0:
            continue
        source = source[begin:end]
        wrapped = re.search(r'master_key_wrapped_user_key:\s*"([^"]+)"', source)
        private = re.search(r'private_key:\s*"([^"]+)"', source)
        if wrapped and private and wrapped.group(1) == PUBLIC_WRAPPED_USER_KEY:
            return private.group(1)
    raise ValueError(
        "Could not locate the pinned SDK's public fixture keys. Restore Cargo dependencies "
        "or pass --sdk-checkout pointing to the pinned SDK checkout."
    )


def _encode(value: dict) -> str:
    return base64.urlsafe_b64encode(json.dumps(value, separators=(",", ":")).encode()).decode().rstrip("=")


class FixtureState:
    def __init__(self, fixture: dict, private_key: str):
        self.lock = threading.RLock()
        self.initial = checked_fixture(fixture)
        # Modern Bitwarden and Vaultwarden include wrapped account keys in the sync profile.
        self.initial["profile"]["accountKeys"] = {
            "publicKeyEncryptionKeyPair": {
                "wrappedPrivateKey": private_key,
                "object": "publicKeyEncryptionKeyPair",
                "Object": "publicKeyEncryptionKeyPair",
            },
            "object": "privateKeys",
            "Object": "privateKeys",
        }
        self.initial["userDecryption"] = {"masterPasswordUnlock": {
            "kdf": {"kdfType": 0, "iterations": 600000},
            "salt": PUBLIC_EMAIL, "masterKeyEncryptedUserKey": PUBLIC_WRAPPED_USER_KEY,
        }}
        self.initial["UserDecryption"] = {
            "MasterPasswordUnlock": {
                "Kdf": {"KdfType": 0, "Iterations": 600000},
                "Salt": PUBLIC_EMAIL, "MasterKeyEncryptedUserKey": PUBLIC_WRAPPED_USER_KEY,
            },
            "WebAuthnPrfOptions": [],
            "UserKeyId": None,
        }
        self._last_revision = dt.datetime(1970, 1, 1, tzinfo=UTC)
        self.token = _encode({"alg": "fixture-only"}) + "." + _encode({
            "exp": int(time.time()) + 3600, "sub": PUBLIC_USER_ID,
            "email": PUBLIC_EMAIL, "scope": ["api", "offline_access"], "jti": str(uuid.uuid4()),
            "premium": True,
        }) + ".fixture"
        self.identity = {
            "access_token": self.token, "expires_in": 3600,
            "refresh_token": "synthetic-refresh-token", "token_type": "Bearer",
            "privateKey": private_key, "key": PUBLIC_WRAPPED_USER_KEY,
            "kdf": 0, "kdfIterations": 600000,
            "userDecryptionOptions": {"masterPasswordUnlock": {
                "kdf": {"kdfType": 0, "iterations": 600000},
                "salt": PUBLIC_EMAIL, "masterKeyEncryptedUserKey": PUBLIC_WRAPPED_USER_KEY,
            }},
        }
        # NodeWarden emits the same decryption options under both spellings.
        self.identity["UserDecryptionOptions"] = copy.deepcopy(self.identity["userDecryptionOptions"])
        self.require_new_device_verification = False
        self.new_device_otp = "246810"
        self.challenged_device_identifier = None
        self.reset()

    def reset(self):
        with self.lock:
            self.sync_template = copy.deepcopy(self.initial)
            self.attachment_blobs = self.sync_template.pop("attachmentBlobs", {})
            self.ciphers = {}
            for cipher in self.sync_template.pop("ciphers"):
                cipher["id"] = str(uuid.UUID(cipher["id"]))
                self._metadata(cipher)
                self.ciphers[cipher["id"]] = cipher
                self._last_revision = max(self._last_revision, parse_date(cipher["revisionDate"]))
            self.folders = {}
            for folder in copy.deepcopy(self.sync_template["folders"]):
                folder["id"] = str(uuid.UUID(folder["id"]))
                folder.setdefault("object", "folder")
                folder.setdefault("revisionDate", "2026-11-20T00:00:00.000Z")
                self.folders[folder["id"]] = folder
                self._last_revision = max(self._last_revision, parse_date(folder["revisionDate"]))
            self.conflict_next_write = False
            self.drop_next_create_response = False
            self.pause_next_create_response = False
            self.reject_folder_batch = False
            self.fail_second_folder_delete = False
            self.fail_next_single_folder_delete = False
            self.fail_next_move_after_first = False
            self.tamper_next_attachment = False
            self.pause_next_attachment = False
            self.fail_next_attachment_upload = False
            self.pause_next_attachment_upload = False
            self.fail_next_attachment_delete = False
            self.use_azure_attachment_upload = False
            self.azure_slots = {}
            self.attachment_downloads = 0
            self.attachment_uploads = 0
            self.fail_next_sync = False
            self.pause_next_sync = False
            self.duplicate_next_sync_cipher = False
            self.sync_requests = 0
            self.password_requests = 0
            self.require_new_device_verification = False
            self.require_authenticator = False
            self.remembered_two_factor = {}
            self.remember_attempts = 0
            self.remember_accepted = 0
            self.remember_invalid_code = False
            self.pause_next_token = False
            self.omit_next_remember_token = False
            self.sends = {}
            self.drop_next_send_response = False
            self.send_blobs = {}
            self.send_upload_started = 0
            self.azure_next_send = False
            self.fail_next_send_upload = False
            self.pause_next_send_upload = False
            self.fail_next_send_delete = False
            self.send_renewals = 0
            self.drop_next_import_response = False
            self.ignore_next_import = False
            self.import_requests = 0
            self.challenged_device_identifier: str | None = None

    def revision(self) -> str:
        # Called while lock is held. Generated revisions stay at least two seconds apart.
        self._last_revision = max(dt.datetime.now(UTC), self._last_revision + dt.timedelta(seconds=2))
        return format_date(self._last_revision)

    @staticmethod
    def _metadata(cipher: dict, previous: dict | None = None):
        previous = previous or {}
        cipher["object"] = "cipherDetails"
        cipher.setdefault("collectionIds", copy.deepcopy(previous.get("collectionIds", [])))
        for field, default in (("edit", True), ("viewPassword", True),
                               ("permissions", {"delete": True, "restore": True})):
            cipher[field] = copy.deepcopy(previous.get(field, cipher.get(field, default)))
            if cipher[field] is None:
                cipher[field] = copy.deepcopy(default)
        cipher.setdefault("favorite", False)
        cipher.setdefault("reprompt", 0)
        cipher.setdefault("organizationUseTotp", previous.get("organizationUseTotp", False))
        cipher.setdefault("creationDate", previous.get("creationDate", "2026-11-20T00:00:00.000Z"))
        cipher.setdefault("revisionDate", cipher["creationDate"])

    def sync(self) -> dict:
        result = copy.deepcopy(self.sync_template)
        result["ciphers"] = copy.deepcopy(list(self.ciphers.values()))
        result["folders"] = copy.deepcopy(list(self.folders.values()))
        return result

    def full_send_write(self, payload: dict, previous: dict | None = None) -> dict:
        kind = payload.get("type")
        content, field = (payload.get("file"), "fileName") if kind == 1 else (payload.get("text"), "text")
        if (kind not in (0, 1) or not isinstance(content, dict)
                or not all(isinstance(value, str) and value.startswith("2.")
                           for value in (payload.get("name"), payload.get("key"), content.get(field)))
                or (previous is not None and previous["type"] != kind)):
            raise ValueError("Send must contain encrypted name, key and content metadata.")
        if payload.get("password"):
            digest = base64.b64decode(payload["password"], validate=True)
            if len(digest) != 32:
                raise ValueError("Send password must be SDK-derived.")
        send_id = uuid.UUID(previous["id"]) if previous else uuid.uuid4()
        result = copy.deepcopy(payload)
        result.update(id=str(send_id), object="send", accessId=base64.urlsafe_b64encode(send_id.bytes).decode().rstrip("="),
                      revisionDate=self.revision(), accessCount=(previous or {}).get("accessCount", 0))
        if kind == 1:
            if previous is None:
                size = result.pop("fileLength", None)
                if not isinstance(size, int) or not 65 <= size <= MAX_BODY_BYTES:
                    raise ValueError("Invalid encrypted Send size.")
                result["file"].update(id=uuid.uuid4().hex, size=str(size), sizeName=f"{size} B")
            elif any(result["file"].get(key) != previous["file"][key] for key in ("id", "size", "sizeName")):
                raise ValueError("Send file cannot be replaced.")
        if previous:
            for field in ("notes", "maxAccessCount", "expirationDate"):
                result.setdefault(field, previous.get(field))
            if result.get("authType") == previous.get("authType"):
                for field in ("password", "emails"):
                    result.setdefault(field, previous.get(field))
        return result

    def full_folder_write(self, payload: dict, folder_id: str | None = None) -> dict:
        name = payload.get("name")
        if not isinstance(name, str) or not name:
            raise ValueError("A folder name is required.")
        return {
            "id": folder_id or str(uuid.uuid4()),
            "name": name,
            "revisionDate": self.revision(),
            "object": "folder",
        }

    def full_write(self, payload: dict, previous: dict | None = None) -> dict:
        kind = payload.get("type")
        if isinstance(kind, bool) or not isinstance(kind, int) or not 1 <= kind <= 8:
            raise ValueError("Invalid cipher type.")
        result = copy.deepcopy(payload)
        for transport in ("encryptedFor", "encryptedByKeyId", "lastKnownRevisionDate", "isOrganizationCipher"):
            result.pop(transport, None)
        result["id"] = previous["id"] if previous else str(uuid.uuid4())
        result["creationDate"] = previous["creationDate"] if previous else self.revision()
        result["revisionDate"] = self.revision()
        result["deletedDate"] = previous.get("deletedDate") if previous else None
        result["archivedDate"] = payload.get("archivedDate")
        result["folderId"] = payload.get("folderId")
        result["favorite"] = payload.get("favorite", False)
        result["notes"] = payload.get("notes")
        old_attachments = {a["id"]: a for a in (previous or {}).get("attachments", []) or []}
        attachments = payload.get("attachments2")
        result.pop("attachments2", None)
        if attachments is not None:
            if not isinstance(attachments, dict):
                raise ValueError("Invalid attachment metadata.")
            updated = copy.deepcopy(old_attachments)
            for key, metadata in attachments.items():
                if not isinstance(metadata, dict):
                    raise ValueError("Invalid attachment metadata.")
                attachment = updated.setdefault(key, {"id": key, "size": "0", "sizeName": "0 B",
                    "url": "https://attachment.example.invalid/synthetic"})
                for field in ("fileName", "key"):
                    attachment[field] = metadata.get(field)
            result["attachments"] = list(updated.values())
        elif previous:
            result["attachments"] = copy.deepcopy(previous.get("attachments"))
        self._metadata(result, previous)
        return result


class FixtureServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    peer_port: int | None = None

    def __init__(self, address: tuple[str, int], state: FixtureState):
        checked_bind(address[0])
        self.state = state
        super().__init__(address, FixtureHandler)

    def handle_error(self, request, client_address):
        # Request data, headers, tokens and local account material must never enter logs.
        pass


class FixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    @property
    def fixture(self) -> FixtureServer:
        return cast(FixtureServer, self.server)

    def loopback_origin(self) -> str:
        host, port = self.fixture.server_address[:2]
        host = str(host)
        return f"http://[{host}]:{port}" if ":" in host else f"http://{host}:{port}"

    def log_message(self, format, *args):
        pass

    def reply(self, value: object = None, status: int = 200):
        body = b"" if status == 204 else json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)

    def fail(self, message: str, status: int = 400):
        self.reply({"message": message, "object": "error"}, status)

    def body(self) -> bytes | None:
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self.fail("Invalid request length.")
            return None
        if length < 0 or length > MAX_BODY_BYTES or self.headers.get("Transfer-Encoding"):
            self.close_connection = True
            self.fail("Unsupported request body.", 413)
            return None
        return self.rfile.read(length)

    def json_body(self, body: bytes) -> dict | None:
        try:
            value = json.loads(body) if body else {}
        except (UnicodeDecodeError, json.JSONDecodeError):
            self.fail("Invalid JSON request.")
            return None
        if not isinstance(value, dict):
            self.fail("Expected a JSON object.")
            return None
        return value

    def authorized(self) -> bool:
        if self.headers.get("Authorization") == "Bearer " + self.fixture.state.token:
            return True
        self.fail("Only the synthetic fixture session is allowed.", 401)
        return False

    def do_GET(self):
        path = urlsplit(self.path).path
        state = self.fixture.state
        if path == "/health":
            peer_port = self.fixture.peer_port
            self.reply({"fixture": True, **({"peerPort": peer_port} if peer_port else {})})
            return
        if path == "/icons/example.com/icon.png":
            icon = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAIAAABvFaqvAAAAKklEQVR4nGMwD2ukCmIYNYh8g170OkIQHpFRg0YNGjVoOBpEKho1iDACAE0Rlr98kVxNAAAAAElFTkSuQmCC")
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(icon)))
            self.end_headers()
            self.wfile.write(icon)
            return
        if path == "/control/send-transfers":
            with state.lock:
                self.reply({"started": state.send_upload_started, "renewals": state.send_renewals})
            return
        fixture_send = re.fullmatch(r"/fixture-send/([0-9a-f-]{36})", path)
        if fixture_send:
            with state.lock:
                data = state.send_blobs.get(fixture_send[1])
            if data is None:
                self.fail("Synthetic Send file not found.", 404)
            else:
                self.reply({"ciphertext": base64.b64encode(data).decode()})
            return
        if path == "/control/snapshot":
            with state.lock:
                self.reply({"cipherCount": len(state.ciphers), "ids": sorted(state.ciphers),
                            "attachmentDownloads": state.attachment_downloads,
                            "attachmentUploads": state.attachment_uploads, "syncRequests": state.sync_requests,
                            "importRequests": state.import_requests, "passwordRequests": state.password_requests,
                            "rememberAttempts": state.remember_attempts,
                            "rememberAccepted": state.remember_accepted})
            return
        blob_match = re.fullmatch(r"/fixture-attachments/([a-zA-Z0-9_-]+)", path)
        if blob_match:
            with state.lock:
                data = state.attachment_blobs.get(blob_match.group(1))
                if data is None or self.headers.get("Authorization") or self.headers.get("Device-Identifier"):
                    self.fail("Invalid synthetic blob request.", 404)
                    return
                contents = base64.b64decode(data, validate=True)
                state.attachment_downloads += 1
                if state.tamper_next_attachment:
                    state.tamper_next_attachment = False
                    contents = contents[:-1] + bytes([contents[-1] ^ 1])
                pause = state.pause_next_attachment
                state.pause_next_attachment = False
            if pause:
                time.sleep(3)  # Allow the native cancellation test to interrupt a live request.
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(contents)))
            self.end_headers()
            self.wfile.write(contents)
            return
        if not self.authorized():
            return
        if path == "/api/sync":
            with state.lock:
                state.sync_requests += 1
                if state.fail_next_sync:
                    state.fail_next_sync = False
                    self.fail("Synthetic sync failure.", 503)
                    return
                pause = state.pause_next_sync
                state.pause_next_sync = False
                response = state.sync()
                if state.duplicate_next_sync_cipher:
                    state.duplicate_next_sync_cipher = False
                    response["ciphers"].extend(response["ciphers"][:1])
            if pause:
                time.sleep(3)
            self.reply(response)
            return
        with state.lock:
            if path == "/api/sends":
                self.reply({"data": list(state.sends.values()), "object": "list"})
                return
            renew = SEND_UPLOAD_PATH.fullmatch(path)
            if renew:
                entry = state.sends.get(renew[1])
                if not entry or (entry.get("file") or {}).get("id") != renew[2]:
                    self.fail("Invalid synthetic renewal.", 404)
                else:
                    state.send_renewals += 1
                    self.reply(self.send_slot(entry, azure=True))
                return
            send_match = re.fullmatch(r"/api/sends/([0-9a-f-]{36})", path)
            if send_match:
                entry = state.sends.get(send_match[1])
                self.reply(entry) if entry else self.fail("Synthetic Send not found.", 404)
                return
            match = re.fullmatch(r"/api/ciphers/([0-9a-fA-F-]{36})", path)
            if match and match.group(1).lower() in state.ciphers:
                self.reply(copy.deepcopy(state.ciphers[match.group(1).lower()]))
                return
            match = re.fullmatch(r"/api/ciphers/([0-9a-fA-F-]{36})/attachment/([a-zA-Z0-9_-]+)", path)
            if match:
                cipher_id, attachment_id = match.groups()
                cipher = state.ciphers.get(cipher_id.lower(), {})
                if (attachment_id in state.attachment_blobs
                        and any(entry.get("id") == attachment_id for entry in cipher.get("attachments") or [])):
                    self.reply({"id": attachment_id, "url": f"{self.loopback_origin()}/fixture-attachments/{attachment_id}"})
                    return
        self.fail("Fixture route or cipher not found.", 404)

    def do_POST(self):
        body = self.body()
        if body is None:
            return
        path = urlsplit(self.path).path
        state = self.fixture.state
        if path in ("/identity/accounts/prelogin/password", "/identity/accounts/prelogin"):
            value = self.json_body(body)
            if value is not None:
                if str(value.get("email", "")).strip().lower() != PUBLIC_EMAIL:
                    self.fail("Only the public synthetic account is accepted.", 403)
                else:
                    self.reply({"kdf": 0, "kdfIterations": 600000,
                                "kdfSettings": {"kdfType": 0, "iterations": 600000, "memory": None, "parallelism": None},
                                "KdfSettings": {"KdfType": 0, "Iterations": 600000, "Memory": None, "Parallelism": None},
                                "salt": None, "Salt": PUBLIC_EMAIL})
            return
        if path == "/identity/connect/token":
            try:
                form = parse_qs(body.decode(), max_num_fields=32)
            except (UnicodeDecodeError, ValueError):
                self.fail("Invalid synthetic token request.")
                return
            grant = form.get("grant_type", [""])[0]
            username = form.get("username", [""])[0].strip().lower()
            refresh = form.get("refresh_token", [""])[0]
            device_identifier = form.get("deviceIdentifier", [""])[0]
            if grant == "password":
                with state.lock:
                    state.password_requests += 1
            if (grant == "password" and username == PUBLIC_EMAIL) or (
                grant == "refresh_token" and refresh == state.identity["refresh_token"]
            ):
                if grant == "password" and state.require_new_device_verification:
                    otp = form.get("newDeviceOtp", form.get("NewDeviceOtp", [""]))[0]
                    if not otp:
                        state.challenged_device_identifier = device_identifier
                        self.reply({"error": "invalid_grant", "error_description": "invalid_grant",
                                    "ErrorModel": {"Message": "new device verification required", "Object": "error"}}, 400)
                        return
                    if otp != state.new_device_otp or device_identifier != state.challenged_device_identifier:
                        self.reply({"error": "invalid_grant", "error_description": "invalid_grant",
                                    "ErrorModel": {"Message": "invalid new device otp", "Object": "error"}}, 400)
                        return
                    state.require_new_device_verification = False
                issued_remember = None
                if grant == "password" and state.require_authenticator:
                    provider = form.get("twoFactorProvider", [""])[0]
                    token = form.get("twoFactorToken", [""])[0]
                    saved = state.remembered_two_factor.get(device_identifier)
                    valid_remember = provider == "5" and saved is not None and token == saved[0]
                    if provider == "5":
                        state.remember_attempts += 1
                        if valid_remember and saved is not None and form.get("password", [""])[0] != saved[1]:
                            self.reply({"error": "invalid_grant", "error_description": "invalid_username_or_password",
                                        "ErrorModel": {"Message": "Username or password is incorrect. Try again.",
                                                       "Object": "error"}}, 400)
                            return
                        if valid_remember:
                            state.remember_accepted += 1
                    if (not provider or not token) or (provider == "5" and not valid_remember and not state.remember_invalid_code):
                        # Legacy self-hosted shape: list and provider data nested in the error.
                        self.reply({"Error": "invalid_grant", "ErrorDescription": "Two factor required.",
                                    "CustomResponse": {"TwoFactorProviders": ["0"], "TwoFactorProviders2": {"0": None}},
                                    "ErrorModel": {"Message": "Two factor required.", "Object": "error"}}, 400)
                        return
                    if not valid_remember and (provider != "0" or token != "087349"):
                        message = "Two-step token is invalid. Try again."
                        self.reply({"error": "invalid_grant", "error_description": message,
                                    "ErrorModel": {"Message": message, "Object": "error"}}, 400)
                        return
                    if provider == "0" and form.get("twoFactorRemember", [""])[0].lower() in ("true", "1"):
                        if state.omit_next_remember_token:
                            state.omit_next_remember_token = False
                        else:
                            issued_remember = "synthetic-remember-" + uuid.uuid4().hex
                            state.remembered_two_factor[device_identifier] = (issued_remember, form.get("password", [""])[0])
                result = copy.deepcopy(state.identity)
                if issued_remember:
                    # Exercise both legacy and modern spellings, as NodeWarden emits aliases.
                    result["twoFactorToken"] = result["TwoFactorToken"] = issued_remember
                if state.pause_next_token:
                    state.pause_next_token = False
                    time.sleep(3)
                self.reply(result)
            else:
                self.fail("Only the public synthetic account is accepted.", 403)
            return
        if path == "/api/accounts/resend-new-device-otp":
            value = self.json_body(body)
            if value is None:
                return
            if (value.get("email") != PUBLIC_EMAIL
                    or not value.get("masterPasswordHash")
                    or not self.headers.get("Device-Identifier")
                    or self.headers.get("Device-Identifier") != state.challenged_device_identifier):
                self.fail("Invalid synthetic device verification resend.", 400)
                return
            self.reply({"ok": True})
            return
        controls = {
            "/control/conflict-next-write": "conflict_next_write",
            "/control/drop-next-write-response": "drop_next_create_response",
            "/control/pause-next-write-response": "pause_next_create_response",
            "/control/reject-folder-batch": "reject_folder_batch",
            "/control/fail-second-folder-delete": "fail_second_folder_delete",
            "/control/fail-next-move-after-first": "fail_next_move_after_first",
            "/control/tamper-next-attachment": "tamper_next_attachment",
            "/control/pause-next-attachment": "pause_next_attachment",
            "/control/fail-next-attachment-upload": "fail_next_attachment_upload",
            "/control/pause-next-attachment-upload": "pause_next_attachment_upload",
            "/control/fail-next-attachment-delete": "fail_next_attachment_delete",
            "/control/azure-next-attachment-upload": "use_azure_attachment_upload",
            "/control/fail-next-sync": "fail_next_sync",
            "/control/pause-next-sync": "pause_next_sync",
            "/control/duplicate-next-sync-cipher": "duplicate_next_sync_cipher",
            "/control/require-authenticator": "require_authenticator",
            "/control/remember-invalid-code": "remember_invalid_code",
            "/control/pause-next-token": "pause_next_token",
            "/control/omit-next-remember-token": "omit_next_remember_token",
            "/control/drop-next-send-response": "drop_next_send_response",
            "/control/azure-next-send": "azure_next_send",
            "/control/fail-next-send-upload": "fail_next_send_upload",
            "/control/pause-next-send-upload": "pause_next_send_upload",
            "/control/fail-next-send-delete": "fail_next_send_delete",
            "/control/drop-next-import-response": "drop_next_import_response",
            "/control/ignore-next-import": "ignore_next_import",
        }
        if path in controls:
            with state.lock:
                setattr(state, controls[path], True)
            self.reply({"ok": True})
            return
        if path == "/control/revoke-remembered-two-factor":
            with state.lock:
                state.remembered_two_factor.clear()
            self.reply({"ok": True})
            return
        if path == "/control/require-new-device-verification":
            with state.lock:
                state.require_new_device_verification = True
            self.reply({"ok": True})
            return
        if path == "/control/change-master-password-wrapper":
            with state.lock:
                # Change the wrapped key reported by sync while keeping the same fixture ciphers.
                wrapper = PUBLIC_WRAPPED_USER_KEY[:-2] + "A="
                state.sync_template["profile"]["key"] = wrapper
                state.sync_template["userDecryption"]["masterPasswordUnlock"]["masterKeyEncryptedUserKey"] = wrapper
                state.sync_template["UserDecryption"]["MasterPasswordUnlock"]["MasterKeyEncryptedUserKey"] = wrapper
            self.reply({"ok": True})
            return
        if path in ("/control/deny-vault-export", "/control/restrict-login-export", "/control/clear-export-policy"):
            with state.lock:
                organization = "00000000-0000-0000-0000-000000000101"
                rows = []
                if path.endswith("deny-vault-export"):
                    rows.append({"type": 10, "enabled": True, "organizationId": organization})
                if path.endswith("restrict-login-export"):
                    rows.append({"type": 15, "enabled": True, "organizationId": organization, "data": [1]})
                state.sync_template["policiesNew"] = rows
            self.reply({"ok": True})
            return
        if path in ("/control/disable-premium", "/control/enable-premium"):
            with state.lock:
                state.sync_template["profile"]["premium"] = path == "/control/enable-premium"
            self.reply({"ok": True})
            return
        if path == "/control/reset":
            state.reset()
            self.reply({"ok": True})
            return
        if path == "/control/stop":
            self.reply({"ok": True})
            threading.Thread(target=self.fixture.shutdown, daemon=True).start()
            return
        if not self.authorized():
            return
        send_upload = SEND_UPLOAD_PATH.fullmatch(path)
        if send_upload:
            content_type = self.headers.get("Content-Type", "")
            if not content_type.startswith("multipart/form-data;"):
                self.fail("Expected a multipart Send upload.")
                return
            message = BytesParser().parsebytes(b"Content-Type: " + content_type.encode() + b"\r\n\r\n" + body)
            parts = message.get_payload()
            if (not isinstance(parts, list) or len(parts) != 1 or not isinstance(parts[0], Message)
                    or parts[0].get_param("name", header="content-disposition") != "data"):
                self.fail("Expected one encrypted data part.")
                return
            data, filename = parts[0].get_payload(decode=True), parts[0].get_filename()
            if not isinstance(data, bytes) or not isinstance(filename, str):
                self.fail("Expected encrypted Send file bytes and filename.")
                return
            self.upload_send_file(send_upload[1], send_upload[2], data, filename)
            return
        attachment = ATTACHMENT_PATH.fullmatch(path)
        if attachment and attachment[2] != "v2":
            content_type = self.headers.get("Content-Type", "")
            if not content_type.startswith("multipart/form-data;"):
                self.fail("Expected a multipart attachment upload.")
                return
            message = BytesParser().parsebytes(b"Content-Type: " + content_type.encode() + b"\r\n\r\n" + body)
            parts = message.get_payload()
            if (not isinstance(parts, list) or len(parts) != 1 or not isinstance(parts[0], Message)
                    or parts[0].get_param("name", header="content-disposition") != "data"):
                self.fail("Expected one encrypted data part.")
                return
            data = parts[0].get_payload(decode=True)
            if not isinstance(data, bytes):
                self.fail("Expected encrypted attachment bytes.")
                return
            self.upload_attachment(attachment[1].lower(), attachment[2], data, 204)
            return
        value = self.json_body(body)
        if value is None:
            return
        if attachment:
            with state.lock:
                cipher = state.ciphers.get(attachment[1].lower())
                if cipher is None or not cipher["edit"] or cipher.get("deletedDate"):
                    self.fail("Attachment upload denied.", 403)
                    return
                try:
                    revision = parse_date(value.get("lastKnownRevisionDate"))
                except ValueError:
                    self.fail("Invalid attachment revision.")
                    return
                if abs((revision - parse_date(cipher["revisionDate"])).total_seconds()) > 1:
                    self.fail(OUT_OF_DATE)
                    return
                size, name, key = value.get("fileSize"), value.get("fileName"), value.get("key")
                if (not isinstance(size, int) or not 0 < size <= MAX_BODY_BYTES
                        or not all(isinstance(field, str) and field.startswith(("2.", "7.")) for field in (name, key))):
                    self.fail("Invalid encrypted attachment metadata.")
                    return
                attachment_id = uuid.uuid4().hex
                cipher["attachments"] = [
                    *(cipher.get("attachments") or []),
                    {"id": attachment_id, "fileName": name, "key": key, "size": str(size), "sizeName": f"{size} B"},
                ]
                cipher["revisionDate"] = state.revision()
                azure = state.use_azure_attachment_upload
                state.use_azure_attachment_upload = False
                if azure:
                    state.azure_slots[attachment_id] = cipher["id"]
                    target = f"{self.loopback_origin()}/fixture-upload/{attachment_id}?sv=2024-11-04"
                else:
                    target = f"/ciphers/{cipher['id']}/attachment/{attachment_id}"
                self.reply({"attachmentId": attachment_id, "url": target,
                            "fileUploadType": 1 if azure else 0, "cipherResponse": cipher})
            return
        if path in ("/api/sends", "/api/sends/file/v2"):
            is_file = path.endswith("/file/v2")
            with state.lock:
                try:
                    if value.get("type") != int(is_file):
                        raise ValueError("Wrong Send creation route.")
                    send = state.full_send_write(value)
                except (ValueError, TypeError):
                    self.fail("Invalid synthetic Send request.")
                    return
                state.sends[send["id"]] = send
                result = self.send_slot(send, state.azure_next_send, expired=True) if is_file else send
                if is_file:
                    state.azure_next_send = False
                drop = state.drop_next_send_response
                state.drop_next_send_response = False
            if drop:
                self.close_connection = True
                try:
                    self.connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                return
            self.reply(result)
            return
        if path == "/api/folders":
            with state.lock:
                try:
                    folder = state.full_folder_write(value)
                except (ValueError, TypeError):
                    self.fail("Invalid synthetic folder request.")
                    return
                state.folders[folder["id"]] = folder
            self.reply(folder)
            return
        if path == "/api/ciphers/import":
            self.import_ciphers(value)
            return
        if path != "/api/ciphers":
            self.fail("Fixture route not found.", 404)
            return
        with state.lock:
            try:
                cipher = state.full_write(value)
            except (ValueError, TypeError):
                self.fail("Invalid synthetic cipher request.")
                return
            state.ciphers[cipher["id"]] = cipher
            drop = state.drop_next_create_response
            pause = state.pause_next_create_response
            state.drop_next_create_response = False
            state.pause_next_create_response = False
        if pause:
            time.sleep(20)  # Allows an integration test to terminate its client before acknowledgment.
        if drop:
            # Commit first, then lose the acknowledgment: callers must synchronize before retrying.
            self.close_connection = True
            try:
                self.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.connection.close()
            return
        self.reply(cipher)

    def import_ciphers(self, value: dict):
        state = self.fixture.state
        with state.lock:
            state.import_requests += 1
            try:
                folder_rows, item_rows, links = (value[key] for key in ("folders", "ciphers", "folderRelationships"))
                if not all(isinstance(rows, list) for rows in (folder_rows, item_rows, links)):
                    raise ValueError("Expected import arrays.")
                if any(not isinstance(row.get("name"), str) or not row["name"].startswith("2.") or row.get("id")
                       for row in folder_rows):
                    raise ValueError("Import must only contain new encrypted folders.")
                if any(not isinstance(row.get("name"), str) or not row["name"].startswith("2.")
                       or row.get("organizationId") or row.get("folderId") or row.get("attachments2")
                       or row.get("encryptedFor") != PUBLIC_USER_ID for row in item_rows):
                    raise ValueError("Import must only contain new encrypted personal items.")
                folders = [state.full_folder_write(row) for row in folder_rows]
                items = [state.full_write(row) for row in item_rows]
                for pair in links:
                    item_index, folder_index = pair["key"], pair["value"]
                    if not 0 <= item_index < len(items) or not 0 <= folder_index < len(folders) or items[item_index]["folderId"]:
                        raise ValueError("Invalid import folder relationship.")
                    items[item_index]["folderId"] = folders[folder_index]["id"]
            except (KeyError, ValueError, TypeError, AttributeError):
                self.fail("Invalid synthetic import.")
                return
            if state.ignore_next_import:
                state.ignore_next_import = False
            else:
                state.folders.update((folder["id"], folder) for folder in folders)
                state.ciphers.update((item["id"], item) for item in items)
            drop = state.drop_next_import_response
            state.drop_next_import_response = False
        if drop:
            self.close_connection = True
            try:
                self.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            return
        self.reply(None, 204)

    def send_slot(self, send: dict, azure: bool, expired: bool = False) -> dict:
        path = f"/api/sends/{send['id']}/file/{send['file']['id']}"
        expiry = "2000-01-01T00:00:00Z" if expired else "2099-01-01T00:00:00Z"
        url = f"{self.loopback_origin()}{path}?token=fixture-signed&sv=2024-11-04&se={expiry}" if azure else path
        return {"sendResponse": send, "fileUploadType": int(azure), "url": url}

    def upload_send_file(self, send_id: str, file_id: str, data: bytes, filename: str | None):
        state = self.fixture.state
        with state.lock:
            entry = state.sends.get(send_id, {}).get("file") or {}
            if (entry.get("id") != file_id or (filename is not None and entry.get("fileName") != filename)
                    or not isinstance(data, bytes) or str(len(data)) != entry.get("size") or not data.startswith(b"\x02")):
                self.fail("Invalid encrypted Send upload.")
                return
            state.send_upload_started += 1
            if state.fail_next_send_upload:
                state.fail_next_send_upload = False
                self.fail("Synthetic Send upload failure.", 503)
                return
            pause = state.pause_next_send_upload
            state.pause_next_send_upload = False
        if pause:
            time.sleep(3)
        with state.lock:
            if send_id not in state.sends:
                self.fail("Send upload was rolled back.", 404)
                return
            state.send_blobs[send_id] = data
        self.reply(None, 201)

    def do_PUT(self):
        body = self.body()
        if body is None:
            return
        path = urlsplit(self.path).path
        azure_attachment = re.fullmatch(r"/fixture-upload/([a-zA-Z0-9_-]+)", path)
        state = self.fixture.state
        send_upload = SEND_UPLOAD_PATH.fullmatch(path)
        if send_upload:
            query = parse_qs(urlsplit(self.path).query)
            if (query.get("token") != ["fixture-signed"] or query.get("se") != ["2099-01-01T00:00:00Z"]
                    or self.headers.get("Authorization") or self.headers.get("Device-Identifier")
                    or self.headers.get("x-ms-blob-type") != "BlockBlob"
                    or self.headers.get("x-ms-version") != "2024-11-04"
                    or self.headers.get("Content-Type") != "application/octet-stream"):
                self.fail("Invalid presigned Send upload.")
                return
            self.upload_send_file(send_upload[1], send_upload[2], body, None)
            return
        if azure_attachment:
            with state.lock:
                cipher_id = state.azure_slots.get(azure_attachment[1])
            if (cipher_id is None or self.headers.get("Authorization") or self.headers.get("Device-Identifier")
                    or self.headers.get("x-ms-blob-type") != "BlockBlob"
                    or self.headers.get("x-ms-version") != "2024-11-04"):
                self.fail("Invalid presigned upload request.")
                return
            self.upload_attachment(cipher_id, azure_attachment[1], body, 201)
            return
        if not self.authorized():
            return
        value = self.json_body(body)
        if value is None:
            return
        send_match = re.fullmatch(r"/api/sends/([0-9a-f-]{36})(/remove-password)?", path)
        if send_match:
            with state.lock:
                send_id, action = send_match.groups()
                previous = state.sends.get(send_id)
                if previous is None:
                    self.fail("Synthetic Send not found.", 404)
                    return
                try:
                    if action:
                        updated = copy.deepcopy(previous)
                        updated.update(password=None, emails=None, authType=2, revisionDate=state.revision())
                    else:
                        updated = state.full_send_write(value, previous)
                except (ValueError, TypeError):
                    self.fail("Invalid synthetic Send edit.")
                    return
                state.sends[send_id] = updated
            self.reply(updated)
            return
        if path == "/api/ciphers/move":
            ids = value.get("ids")
            folder_id = value.get("folderId")
            with state.lock:
                if (not isinstance(ids, list) or not 1 <= len(ids) <= 500
                        or not all(isinstance(key, str) and key in state.ciphers for key in ids)
                        or (folder_id is not None and (not isinstance(folder_id, str) or folder_id not in state.folders))):
                    self.fail("Invalid synthetic item move.")
                    return
                for cipher_id in ids:
                    state.ciphers[cipher_id]["folderId"] = folder_id
                    if state.fail_next_move_after_first:
                        state.fail_next_move_after_first = False
                        self.fail("Only one of the selected items was moved.")
                        return
            self.reply(None, 204)
            return
        folder_match = re.fullmatch(r"/api/folders/([0-9a-fA-F-]{36})", path)
        if folder_match:
            folder_id = folder_match.group(1).lower()
            with state.lock:
                if folder_id not in state.folders:
                    self.fail("Synthetic folder not found.", 404)
                    return
                try:
                    updated = state.full_folder_write(value, folder_id)
                except (ValueError, TypeError):
                    self.fail("Invalid synthetic folder request.")
                    return
                state.folders[folder_id] = updated
            self.reply(updated)
            return
        if path in ("/api/ciphers/delete", "/api/ciphers/restore"):
            ids = value.get("ids")
            if not isinstance(ids, list) or not 1 <= len(ids) <= 500 or not all(isinstance(key, str) for key in ids):
                self.fail("Invalid synthetic cipher batch.")
                return
            norm_ids = [key.lower() for key in ids]
            permission = "delete" if path == "/api/ciphers/delete" else "restore"
            with state.lock:
                for key in norm_ids:
                    cipher = state.ciphers.get(key)
                    if cipher is None:
                        self.fail("Synthetic cipher not found.", 404)
                        return
                    if not cipher["permissions"].get(permission, False):
                        self.fail(f"Synthetic {permission} permission denied.", 403)
                        return
                revision = state.revision()
                updated_items = []
                for key in dict.fromkeys(norm_ids):
                    updated = copy.deepcopy(state.ciphers[key])
                    updated["revisionDate"] = revision
                    updated["deletedDate"] = revision if path == "/api/ciphers/delete" else None
                    state.ciphers[key] = updated
                    updated_items.append(updated)
            if path == "/api/ciphers/delete":
                self.reply(None, 204)
            else:
                self.reply({"object": "list", "data": updated_items})
            return
        match = re.fullmatch(r"/api/ciphers/([0-9a-fA-F-]{36})(?:/(partial|delete|restore|archive|unarchive))?", path)
        if not match:
            self.fail("Fixture route not found.", 404)
            return
        key, action = match.group(1).lower(), match.group(2)
        with state.lock:
            previous = state.ciphers.get(key)
            if previous is None:
                self.fail("Synthetic cipher not found.", 404)
                return
            if state.conflict_next_write:
                state.conflict_next_write = False
                previous["revisionDate"] = state.revision()
                self.fail(OUT_OF_DATE)
                return
            if action is None:
                try:
                    expected = parse_date(value.get("lastKnownRevisionDate"))
                except ValueError:
                    self.fail("A valid lastKnownRevisionDate is required.")
                    return
                if abs((expected - parse_date(previous["revisionDate"])).total_seconds()) > 1:
                    self.fail(OUT_OF_DATE)
                    return
                if not previous["edit"] or previous.get("partialData") or previous.get("deletedDate"):
                    self.fail("Synthetic cipher is not editable.", 403)
                    return
                try:
                    updated = state.full_write(value, previous)
                except (ValueError, TypeError):
                    self.fail("Invalid synthetic cipher request.")
                    return
            else:
                updated = copy.deepcopy(previous)
                permissions = previous["permissions"]
                if action == "delete" and not permissions.get("delete", False):
                    self.fail("Synthetic delete permission denied.", 403)
                    return
                if action == "restore" and not permissions.get("restore", False):
                    self.fail("Synthetic restore permission denied.", 403)
                    return
                revision = state.revision()
                updated["revisionDate"] = revision
                if action == "partial":
                    updated["favorite"] = value.get("favorite", False)
                    updated["folderId"] = value.get("folderId")
                elif action == "delete":
                    updated["deletedDate"] = revision
                elif action == "restore":
                    updated["deletedDate"] = None
                elif action == "archive":
                    updated["archivedDate"] = revision
                elif action == "unarchive":
                    updated["archivedDate"] = None
            state.ciphers[key] = updated
        self.reply(None, 204) if action == "delete" else self.reply(updated)

    def do_DELETE(self):
        if not self.authorized():
            return
        state = self.fixture.state
        path = urlsplit(self.path).path
        send_match = re.fullmatch(r"/api/sends/([0-9a-f-]{36})", path)
        if send_match:
            with state.lock:
                if state.fail_next_send_delete:
                    state.fail_next_send_delete = False
                    self.fail("Synthetic Send delete failed.", 503)
                    return
                removed = state.sends.pop(send_match[1], None)
                state.send_blobs.pop(send_match[1], None)
            self.reply(None, 204) if removed else self.fail("Synthetic Send not found.", 404)
            return
        attachment = ATTACHMENT_PATH.fullmatch(path)
        if attachment:
            cipher_id, attachment_id = attachment[1].lower(), attachment[2]
            with state.lock:
                if state.fail_next_attachment_delete:
                    state.fail_next_attachment_delete = False
                    self.fail("Synthetic attachment delete failed.", 503)
                    return
                cipher = state.ciphers.get(cipher_id)
                if cipher is None or not cipher["permissions"].get("delete", False):
                    self.fail("Attachment delete denied.", 403)
                    return
                current = cipher.get("attachments") or []
                kept = [entry for entry in current if entry["id"] != attachment_id]
                if len(kept) == len(current):
                    self.fail("Attachment not found.", 404)
                    return
                cipher["attachments"] = kept
                cipher["revisionDate"] = state.revision()
                state.attachment_blobs.pop(attachment_id, None)
                state.azure_slots.pop(attachment_id, None)
                self.reply({"cipher": cipher})
            return
        folder_match = re.fullmatch(r"/api/folders/([0-9a-fA-F-]{36})", path)
        if path == "/api/folders" or folder_match:
            if folder_match:
                ids = [folder_match.group(1).lower()]
            else:
                body = self.body()
                value = self.json_body(body) if body is not None else None
                if value is None:
                    return
                ids = value.get("ids")
                if not isinstance(ids, list) or not 1 <= len(ids) <= 500 or not all(isinstance(key, str) for key in ids):
                    self.fail("Invalid synthetic folder batch.")
                    return
            with state.lock:
                if not folder_match and state.reject_folder_batch:
                    state.reject_folder_batch = False
                    self.fail("Fixture bulk route unavailable.", 405)
                    return
                if any(key not in state.folders for key in ids):
                    self.fail("Synthetic folder not found.", 404)
                    return
                if folder_match and state.fail_next_single_folder_delete:
                    state.fail_next_single_folder_delete = False
                    self.fail("Synthetic folder delete failed.", 503)
                    return
                if folder_match and state.fail_second_folder_delete:
                    state.fail_second_folder_delete = False
                    state.fail_next_single_folder_delete = True
                deleted = set(ids)
                for folder_id in deleted:
                    del state.folders[folder_id]
                for cipher in state.ciphers.values():
                    if cipher.get("folderId") in deleted:
                        cipher["folderId"] = None
            self.reply(None, 204)
            return
        if path == "/api/ciphers":
            body = self.body()
            value = self.json_body(body) if body is not None else None
            if value is None:
                return
            ids = value.get("ids")
            if not isinstance(ids, list) or not 1 <= len(ids) <= 500 or not all(isinstance(key, str) for key in ids):
                self.fail("Invalid synthetic cipher batch.")
                return
            norm_ids = [key.lower() for key in ids]
            with state.lock:
                for key in norm_ids:
                    cipher = state.ciphers.get(key)
                    if cipher is None:
                        self.fail("Synthetic cipher not found.", 404)
                        return
                    if not cipher["permissions"].get("delete", False):
                        self.fail("Synthetic delete permission denied.", 403)
                        return
                for key in dict.fromkeys(norm_ids):
                    del state.ciphers[key]
            self.reply(None, 204)
            return
        match = re.fullmatch(r"/api/ciphers/([0-9a-fA-F-]{36})", path)
        with state.lock:
            if not match or match.group(1).lower() not in state.ciphers:
                self.fail("Synthetic cipher not found.", 404)
                return
            key = match.group(1).lower()
            if not state.ciphers[key]["permissions"].get("delete", False):
                self.fail("Synthetic delete permission denied.", 403)
                return
            del state.ciphers[key]
        self.reply(None, 204)

    def upload_attachment(self, cipher_id, attachment_id, contents, status):
        state = self.fixture.state
        with state.lock:
            cipher = state.ciphers.get(cipher_id, {})
            attachment = next((entry for entry in cipher.get("attachments") or [] if entry["id"] == attachment_id), None)
            if attachment is None or not contents or len(contents) != int(attachment["size"]) or contents[0] not in (2, 7):
                self.fail("Invalid synthetic encrypted file upload.")
                return
            state.attachment_blobs[attachment_id] = base64.b64encode(contents).decode("ascii")
            state.attachment_uploads += 1
            if state.fail_next_attachment_upload:
                state.fail_next_attachment_upload = False
                self.fail("Synthetic failure after storing the uploaded bytes.", 503)
                return
            pause = state.pause_next_attachment_upload
            state.pause_next_attachment_upload = False
        if pause:
            time.sleep(3)
        self.reply(None, status)


def make_server(host: str, port: int, state: FixtureState) -> FixtureServer:
    host = checked_bind(host)
    server_type = FixtureServer
    if ipaddress.ip_address(host).version == 6:
        class IPv6FixtureServer(FixtureServer):
            address_family = socket.AF_INET6
        server_type = IPv6FixtureServer
    return server_type((host, port), state)


def self_test() -> bool:
    class HttpTests(unittest.TestCase):
        def setUp(self):
            fixture = {"profile": {"id": PUBLIC_USER_ID, "key": PUBLIC_WRAPPED_USER_KEY},
                       "ciphers": [], "folders": []}
            self.state = FixtureState(fixture, "synthetic-public-private-key-fixture")
            self.server = make_server("127.0.0.1", 0, self.state)
            self.thread = threading.Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
            self.thread.start()

        def tearDown(self):
            self.server.shutdown()
            self.server.server_close()
            self.thread.join(timeout=2)

        def request(self, method, path, data=None, *, raw=None, authenticated=True, device_identifier=None) -> tuple[int, dict[str, Any]]:
            connection = http.client.HTTPConnection("127.0.0.1", self.server.server_port, timeout=3)
            headers = {"Content-Type": "application/json"}
            if authenticated:
                headers["Authorization"] = "Bearer " + self.state.token
            if device_identifier is not None:
                headers["Device-Identifier"] = device_identifier
            body = raw if raw is not None else json.dumps(data).encode() if data is not None else b""
            try:
                connection.request(method, path, body=body, headers=headers)
                response = connection.getresponse()
                body = response.read()
                value = json.loads(body) if body else {}
                if not isinstance(value, dict):
                    raise ValueError("Expected a fixture response object.")
                return response.status, value
            finally:
                connection.close()

        def create(self, **extra):
            payload = {"type": 1, "name": "opaque-synthetic-ciphertext", "favorite": False, **extra}
            status, value = self.request("POST", "/api/ciphers", payload)
            self.assertEqual(status, 200)
            return value

        def test_fixture_and_bind_guards(self):
            with self.assertRaises(ValueError):
                make_server("0.0.0.0", 0, self.state)
            for profile in ({"id": str(uuid.uuid4()), "key": PUBLIC_WRAPPED_USER_KEY},
                            {"id": PUBLIC_USER_ID, "key": "not-the-public-key"}):
                with self.assertRaises(ValueError):
                    checked_fixture({"profile": profile, "ciphers": [], "folders": []})

        def test_only_public_account_can_log_in_and_api_requires_fixture_token(self):
            status, _ = self.request("POST", "/identity/accounts/prelogin/password", {"email": "not-public@example.invalid"}, authenticated=False)
            self.assertEqual(status, 403)
            status, _ = self.request("POST", "/identity/connect/token", raw=b"grant_type=password&username=not-public%40example.invalid", authenticated=False)
            self.assertEqual(status, 403)
            status, identity = self.request("POST", "/identity/connect/token", raw=b"grant_type=password&username=test%40bitwarden.com", authenticated=False)
            self.assertEqual(status, 200)
            self.assertEqual(identity["access_token"], self.state.token)
            self.assertEqual(self.request("GET", "/api/sync", authenticated=False)[0], 401)
            profile = self.request("GET", "/api/sync")[1]["profile"]
            self.assertTrue(profile["premium"])
            self.assertEqual(profile["accountKeys"]["publicKeyEncryptionKeyPair"]["wrappedPrivateKey"],
                             "synthetic-public-private-key-fixture")

        def test_full_edit_and_revision_conflict(self):
            original = self.create(notes="old-encrypted-notes")
            edit = {"type": 1, "name": "changed-encrypted-name", "lastKnownRevisionDate": original["revisionDate"]}
            status, updated = self.request("PUT", "/api/ciphers/" + original["id"], edit)
            self.assertEqual(status, 200)
            self.assertEqual(updated["creationDate"], original["creationDate"])
            self.assertIsNone(updated["notes"])
            self.assertGreaterEqual((parse_date(updated["revisionDate"]) - parse_date(original["revisionDate"])).total_seconds(), 2)
            status, error = self.request("PUT", "/api/ciphers/" + original["id"], edit)
            self.assertEqual(status, 400)
            self.assertEqual(error["message"], OUT_OF_DATE)
            self.assertEqual(self.request("GET", "/api/ciphers/" + original["id"])[1]["name"], "changed-encrypted-name")

        def test_metadata_and_deletion_lifecycle(self):
            item = self.create(data="opaque-encrypted-blob")
            path = "/api/ciphers/" + item["id"]
            status, value = self.request("PUT", path + "/partial", {"favorite": True})
            self.assertEqual(status, 200)
            self.assertTrue(value["favorite"])
            self.assertEqual(value["data"], "opaque-encrypted-blob")
            self.assertIsNotNone(self.request("PUT", path + "/archive")[1]["archivedDate"])
            self.assertIsNone(self.request("PUT", path + "/unarchive")[1]["archivedDate"])
            self.assertEqual(self.request("PUT", path + "/delete")[0], 204)
            self.assertIsNotNone(self.request("GET", path)[1]["deletedDate"])
            self.assertIsNone(self.request("PUT", path + "/restore")[1]["deletedDate"])
            self.assertEqual(self.request("DELETE", path)[0], 204)
            self.assertEqual(self.request("GET", "/control/snapshot")[1]["cipherCount"], 0)

        def test_bulk_trash_restore_and_permanent_delete(self):
            first = self.create(name="first-bulk-item")
            second = self.create(name="second-bulk-item")
            ids = [first["id"], second["id"]]
            self.assertEqual(self.request("PUT", "/api/ciphers/delete", {"ids": ids})[0], 204)
            for item_id in ids:
                self.assertIsNotNone(self.request("GET", "/api/ciphers/" + item_id)[1]["deletedDate"])
            status, restored = self.request("PUT", "/api/ciphers/restore", {"ids": ids})
            self.assertEqual(status, 200)
            self.assertEqual(restored["object"], "list")
            self.assertEqual([entry["id"] for entry in restored["data"]], ids)
            for entry in restored["data"]:
                self.assertIsNone(entry["deletedDate"])
            self.assertEqual(self.request("DELETE", "/api/ciphers", {"ids": ids})[0], 204)
            self.assertEqual(self.request("GET", "/control/snapshot")[1]["cipherCount"], 0)
            self.assertEqual(self.request("PUT", "/api/ciphers/delete", {"ids": [str(uuid.uuid4())]})[0], 404)

        def test_folder_create_edit_delete_preserves_and_uncategorizes_ciphers(self):
            created_name = "encrypted-folder-name"
            status, folder = self.request("POST", "/api/folders", {"name": created_name})
            self.assertEqual(status, 200)
            self.assertEqual(folder["name"], created_name)
            folder_id = folder["id"]
            item = self.create(folderId=folder_id)

            status, updated = self.request("PUT", "/api/folders/" + folder_id, {"name": "renamed-encrypted-folder"})
            self.assertEqual(status, 200)
            self.assertEqual(updated["name"], "renamed-encrypted-folder")
            self.assertEqual(self.request("GET", "/api/sync")[1]["folders"], [updated])

            status, _ = self.request("DELETE", "/api/folders/" + folder_id)
            self.assertEqual(status, 204)
            sync = self.request("GET", "/api/sync")[1]
            self.assertEqual(sync["folders"], [])
            retained = next(cipher for cipher in sync["ciphers"] if cipher["id"] == item["id"])
            self.assertIsNone(retained["folderId"])

        def test_unknown_folder_mutations_fail_without_changing_sync(self):
            missing = str(uuid.uuid4())
            self.assertEqual(self.request("PUT", "/api/folders/" + missing, {"name": "encrypted-name"})[0], 404)
            self.assertEqual(self.request("DELETE", "/api/folders/" + missing)[0], 404)
            self.assertEqual(self.request("POST", "/api/folders", {"name": ""})[0], 400)
            self.assertEqual(self.request("GET", "/api/sync")[1]["folders"], [])

        def test_controls_and_committed_but_dropped_create_response(self):
            item = self.create()
            self.request("POST", "/control/conflict-next-write")
            status, error = self.request("PUT", "/api/ciphers/" + item["id"] + "/partial", {"favorite": True})
            self.assertEqual((status, error["message"]), (400, OUT_OF_DATE))
            self.assertNotEqual(self.request("GET", "/api/ciphers/" + item["id"])[1]["revisionDate"], item["revisionDate"])
            self.request("POST", "/control/fail-next-sync")
            self.assertEqual(self.request("GET", "/api/sync")[0], 503)
            self.assertEqual(self.request("GET", "/api/sync")[0], 200)
            self.request("POST", "/control/drop-next-write-response")
            with self.assertRaises((http.client.RemoteDisconnected, ConnectionResetError)):
                self.request("POST", "/api/ciphers", {"type": 2, "name": "opaque-committed-name"})
            self.assertEqual(self.request("GET", "/control/snapshot")[1]["cipherCount"], 2)
            self.request("POST", "/control/reset")
            self.assertEqual(self.request("GET", "/control/snapshot")[1]["cipherCount"], 0)

        def test_new_device_verification_challenge_and_resend(self):
            self.request("POST", "/control/require-new-device-verification")
            device_identifier = str(uuid.uuid4())
            challenge = ("grant_type=password&username=test%40bitwarden.com&deviceIdentifier="
                         + device_identifier).encode()
            status, response = self.request("POST", "/identity/connect/token", raw=challenge, authenticated=False)
            self.assertEqual(status, 400)
            self.assertEqual(response["ErrorModel"]["Message"], "new device verification required")
            bad = challenge + b"&newDeviceOtp=wrong"
            status, response = self.request("POST", "/identity/connect/token", raw=bad, authenticated=False)
            self.assertEqual(status, 400)
            self.assertEqual(response["ErrorModel"]["Message"], "invalid new device otp")
            valid = challenge + b"&newDeviceOtp=246810"
            status, response = self.request("POST", "/identity/connect/token", raw=valid, authenticated=False)
            self.assertEqual(status, 200)
            self.assertEqual(response["access_token"], self.state.token)
            status, response = self.request("POST", "/api/accounts/resend-new-device-otp",
                                            {"email": PUBLIC_EMAIL, "masterPasswordHash": "synthetic-hash"},
                                            authenticated=False, device_identifier=device_identifier)
            self.assertEqual(status, 200)

        def test_remembered_auth_still_requires_a_matching_password_hash(self):
            self.state.require_authenticator = True
            self.state.remembered_two_factor["synthetic-device"] = ("synthetic-token", "expected-hash")
            form = (b"grant_type=password&username=test%40bitwarden.com&deviceIdentifier=synthetic-device"
                    b"&twoFactorProvider=5&twoFactorToken=synthetic-token&password=wrong-hash")
            status, response = self.request("POST", "/identity/connect/token", raw=form, authenticated=False)
            self.assertEqual(status, 400)
            self.assertEqual(response["error"], "invalid_grant")
            self.assertEqual(response["ErrorModel"]["Object"], "error")
            self.assertEqual(self.state.remember_accepted, 0)

        def test_attachment_request_metadata_is_converted_and_preserved(self):
            item = self.create(attachments2={"synthetic-attachment": {"fileName": "encrypted-name", "key": "encrypted-key"}})
            self.assertEqual(item["attachments"][0]["key"], "encrypted-key")
            payload = {"type": 1, "name": "new-name", "lastKnownRevisionDate": item["revisionDate"]}
            updated = self.request("PUT", "/api/ciphers/" + item["id"], payload)[1]
            self.assertEqual(updated["attachments"], item["attachments"])

    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(HttpTests))
    return result.wasSuccessful()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fixture", help="JSON output from the ignored Rust synthetic fixture exporter")
    parser.add_argument("--port", type=int, default=0, help="Loopback port; 0 asks the OS for a free port")
    parser.add_argument("--bind", default="127.0.0.1", help="Literal loopback address only")
    parser.add_argument("--sdk-checkout", help="Pinned sdk-internal checkout root")
    parser.add_argument("--port-file", help="Write the selected port as a decimal number to this file")
    parser.add_argument("--peer", action="store_true", help="Start a second independent loopback fixture for account switching")
    parser.add_argument("--self-test", action="store_true", help="Run the isolated stdlib HTTP/state tests")
    args = parser.parse_args()
    if args.self_test:
        return 0 if self_test() else 1
    if not args.fixture:
        parser.error("--fixture is required unless --self-test is used")
    try:
        if not 0 <= args.port <= 65535:
            raise ValueError("The port must be between 0 and 65535.")
        host = checked_bind(args.bind)
        fixture = checked_fixture(json.loads(Path(args.fixture).read_text(encoding="utf-8-sig")))
        private_key = load_public_private_key(args.sdk_checkout)
        state = FixtureState(fixture, private_key)
        server = make_server(host, args.port, state)
        peer = make_server(host, 0, FixtureState(fixture, private_key)) if args.peer else None
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))
    if peer:
        server.peer_port = peer.server_port
        threading.Thread(target=peer.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True).start()
    try:
        if args.port_file:
            port_file = Path(args.port_file)
            port_file.parent.mkdir(parents=True, exist_ok=True)
            pending = port_file.with_name(f".{port_file.name}.{uuid.uuid4().hex}.tmp")
            try:
                pending.write_text(str(server.server_port) + "\n", encoding="ascii")
                pending.replace(port_file)
            finally:
                pending.unlink(missing_ok=True)
        print(json.dumps({"fixture": True, "host": host, "port": server.server_port}, separators=(",", ":")), flush=True)
        server.serve_forever(poll_interval=0.1)
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        if peer:
            peer.shutdown()
            peer.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
