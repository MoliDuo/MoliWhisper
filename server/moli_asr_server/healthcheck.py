"""Exit 0 when the server on this machine answers ``/healthz`` (for Docker)."""

from __future__ import annotations

import os
import sys
import urllib.request


def main() -> int:
    port = os.environ.get("MOLI_ASR_PORT", "8765")
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/healthz",
        headers={"Authorization": f"Bearer {os.environ.get('MOLI_ASR_TOKEN', '')}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=4) as response:
            return 0 if response.status == 200 else 1
    except OSError:
        return 1


if __name__ == "__main__":
    sys.exit(main())
