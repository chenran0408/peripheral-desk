"""Check ABI 1 buffers/JSON in isolated child processes; never query hardware."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import subprocess
import sys

MAX_REPLY = 1024 * 1024


def check_one(path: Path) -> None:
    if os.name != "nt" or ctypes.sizeof(ctypes.c_void_p) != 8:
        raise RuntimeError("ABI checks require 64-bit Windows Python")

    class Buffer(ctypes.Structure):
        _fields_ = [("ptr", ctypes.POINTER(ctypes.c_ubyte)), ("len", ctypes.c_size_t)]

    library = ctypes.CDLL(str(path.resolve()))
    abi = library.pd_driver_abi
    abi.argtypes = []
    abi.restype = ctypes.c_uint32
    if abi() != 1:
        raise RuntimeError("Unsupported driver ABI")
    request = library.pd_driver_request
    request.argtypes = [ctypes.POINTER(ctypes.c_ubyte), ctypes.c_size_t]
    request.restype = Buffer
    release = library.pd_driver_free
    release.argtypes = [ctypes.POINTER(ctypes.c_ubyte), ctypes.c_size_t]
    release.restype = None
    checks = {
        "unknown-operation": b'{"op":"abi-self-test-unknown"}',
        "invalid-json": b"{",
        "invalid-utf8": bytes([255, 254]),
    }
    for name, payload in checks.items():
        raw = (ctypes.c_ubyte * len(payload)).from_buffer_copy(payload)
        response = request(raw, len(payload))
        try:
            if not response.ptr or not 0 < response.len <= MAX_REPLY:
                raise RuntimeError(f"{name}: invalid returned buffer")
            reply = json.loads(ctypes.string_at(response.ptr, response.len).decode("utf-8"))
            if reply.get("ok") is not False or reply.get("value") is not None:
                raise RuntimeError(f"{name}: request was unexpectedly accepted")
            if not isinstance(reply.get("error"), str) or not reply["error"]:
                raise RuntimeError(f"{name}: missing error message")
        finally:
            if response.ptr:
                release(response.ptr, response.len)
    print(json.dumps({"dll": path.name, "abi": 1, "checks": list(checks)}, ensure_ascii=False))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dll", nargs="+", type=Path)
    parser.add_argument("--child", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.child:
        check_one(args.dll[0])
        return 0
    for path in args.dll:
        result = subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), "--child", str(path.resolve())],
            capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=15,
            env={**os.environ, "PYTHONUTF8": "1"},
        )
        if result.returncode:
            print(f"ABI check failed: {path}\n{result.stderr or result.stdout}", file=sys.stderr)
            return 1
        print(result.stdout, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
