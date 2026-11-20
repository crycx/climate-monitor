#!/usr/bin/env python3
"""Publish firmware for over-the-air updates.

Converts the built ELF to an app image (`espflash save-image`), checks that
it is an image of the chosen project for its chip, and publishes it retained
to the MQTT broker the gateways use:

    <base>/ota/<project>/image   the image (binary)
    <base>/ota/<project>/meta    {"version", "size", "sha256"}

Projects: xiao-c6-sensor (the ESP-NOW battery sensors, default), c6 and c3
(the gateways). The Firmware entity of each such device in Home Assistant
then shows the version as "latest"; "Install" there updates that device (see
shared/gateway_ota.rs). Only the Python standard library is needed.

    cd esp32/xiao-c6-sensor && cargo build --release
    python ../tools/publish_ota.py --host broker.example --user esp32
    python ../tools/publish_ota.py --project c6 --host broker.example --user esp32

The password is read from MQTT_PASSWORD or asked for. `--clear` removes
the published image again.
"""

import argparse
import getpass
import hashlib
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile

# project: (espflash chip, image header chip id, Rust target)
PROJECTS = {
    "xiao-c6-sensor": ("esp32c6", 13, "riscv32imac-unknown-none-elf"),
    "c6": ("esp32c6", 13, "riscv32imac-unknown-none-elf"),
    "c3": ("esp32c3", 5, "riscv32imc-unknown-none-elf"),
}
APP_DESC_OFFSET = 32
APP_DESC_MAGIC = 0xABCD5432
MAX_VERSION_LEN = 16
ESP32_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")


def default_elf(project: str) -> str:
    """The release build: esp32/<project>/target/<target>/release/<project>."""
    return os.path.join(ESP32_DIR, project, "target", PROJECTS[project][2], "release", project)


def cstr(raw: bytes) -> str:
    return raw.split(b"\0", 1)[0].decode("utf-8", "replace")


def check_image(image: bytes, project: str) -> str:
    """Returns the firmware version from the app descriptor."""
    chip, chip_id_expected, _ = PROJECTS[project]
    if len(image) < APP_DESC_OFFSET + 256 or image[0] != 0xE9:
        sys.exit("not an ESP app image")
    chip_id = struct.unpack_from("<H", image, 12)[0]
    if chip_id != chip_id_expected:
        sys.exit(f"image is for chip id {chip_id}, not {chip}")
    magic = struct.unpack_from("<I", image, APP_DESC_OFFSET)[0]
    if magic != APP_DESC_MAGIC:
        sys.exit("image has no app descriptor at offset 32")
    version = cstr(image[APP_DESC_OFFSET + 16:APP_DESC_OFFSET + 48])
    name = cstr(image[APP_DESC_OFFSET + 48:APP_DESC_OFFSET + 80])
    if name != project:
        sys.exit(f"image is for project '{name}', not '{project}'")
    if not version or len(version.encode()) > MAX_VERSION_LEN:
        sys.exit(f"version '{version}' must be 1..{MAX_VERSION_LEN} bytes")
    return version


def build_image(elf: str, chip: str) -> bytes:
    if not os.path.isfile(elf):
        sys.exit(f"{elf} not found (cargo build --release first)")
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "image.bin")
        cmd = ["espflash", "save-image", "--chip", chip, elf, out]
        result = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace")
        if result.returncode != 0:
            sys.exit("espflash save-image failed:\n" + result.stdout + result.stderr)
        with open(out, "rb") as f:
            return f.read()


# ── Minimal MQTT 3.1.1 client (CONNECT, PUBLISH QoS 0, DISCONNECT) ──────────

def remaining_length(n: int) -> bytes:
    out = bytearray()
    while True:
        byte, n = n % 128, n // 128
        out.append(byte | (0x80 if n else 0))
        if not n:
            return bytes(out)


def utf8(s: str) -> bytes:
    raw = s.encode()
    return struct.pack(">H", len(raw)) + raw


def mqtt_connect(sock: socket.socket, client_id: str, user: str, password: str) -> None:
    flags = 0x02  # clean session
    payload = utf8(client_id)
    if user:
        flags |= 0x80
        payload += utf8(user)
        if password:
            flags |= 0x40
            payload += utf8(password)
    body = utf8("MQTT") + bytes([4, flags]) + struct.pack(">H", 60) + payload
    sock.sendall(b"\x10" + remaining_length(len(body)) + body)
    connack = sock.recv(4)
    if len(connack) != 4 or connack[0] != 0x20 or connack[3] != 0:
        codes = {1: "protocol", 2: "client id", 3: "server unavailable", 4: "bad user/password", 5: "not authorized"}
        code = connack[3] if len(connack) == 4 else None
        sys.exit(f"MQTT connect refused: {codes.get(code, connack.hex())}")


def mqtt_publish(sock: socket.socket, topic: str, payload: bytes) -> None:
    body = utf8(topic) + payload
    sock.sendall(b"\x31" + remaining_length(len(body)) + body)  # QoS 0, retained


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--project", default="xiao-c6-sensor", choices=sorted(PROJECTS),
                    help="firmware to publish (default: xiao-c6-sensor)")
    ap.add_argument("--elf", help="built firmware (default: the project's release build)")
    ap.add_argument("--host", help="MQTT broker (the gateways' mqtt_host)")
    ap.add_argument("--port", type=int, default=1883)
    ap.add_argument("--user", default="", help="MQTT username")
    ap.add_argument("--base", default="climate_monitor", help="the gateways' mqtt_base_topic")
    ap.add_argument("--allow-dirty", action="store_true",
                    help="publish a '-dirty' build (its version does not identify the code)")
    ap.add_argument("--clear", action="store_true", help="remove the published image")
    ap.add_argument("--out", help="also write the app image to this file")
    ap.add_argument("--dry-run", action="store_true", help="build and check only, do not publish")
    args = ap.parse_args()
    if not args.host and not args.dry_run:
        ap.error("--host is required (or use --dry-run)")

    password = os.environ.get("MQTT_PASSWORD")
    if password is None and args.user and not args.dry_run:
        password = getpass.getpass(f"MQTT password for {args.user}@{args.host}: ")

    project = args.project
    image_topic = f"{args.base}/ota/{project}/image"
    meta_topic = f"{args.base}/ota/{project}/meta"

    if args.clear:
        image = meta = b""
    else:
        image = build_image(args.elf or default_elf(project), PROJECTS[project][0])
        version = check_image(image, project)
        if version.endswith("-dirty") and not args.allow_dirty:
            sys.exit(f"version '{version}' has uncommitted changes: commit first, build a test "
                     "image with FW_VERSION=<name>, or pass --allow-dirty")
        sha256 = hashlib.sha256(image).hexdigest()
        meta = json.dumps({"version": version, "size": len(image), "sha256": sha256},
                          separators=(",", ":")).encode()
        print(f"{project} {version}: {len(image)} bytes, sha256 {sha256}")
        if args.out:
            with open(args.out, "wb") as f:
                f.write(image)
            print(f"Image written to {args.out}")
        if args.dry_run:
            print("Dry run: not published")
            return

    with socket.create_connection((args.host, args.port), timeout=30) as sock:
        mqtt_connect(sock, f"{args.base}-publish-ota-{os.getpid()}", args.user, password or "")
        # Image first: a gateway that sees the new meta must find its image.
        mqtt_publish(sock, image_topic, image)
        mqtt_publish(sock, meta_topic, meta)
        sock.sendall(b"\xe0\x00")  # DISCONNECT

    if args.clear:
        print(f"Removed {image_topic} and {meta_topic}")
    else:
        print(f"Published to {image_topic}")
        print("In Home Assistant, the devices' Firmware entity shows the update after their next report.")


if __name__ == "__main__":
    main()
