#!/usr/bin/env python3
"""Content-defined chunking of two files; how much of the second is already in the first.

    chunk-delta.py OLD NEW

Gear hash, 256 KiB min / ~1 MiB average / 4 MiB max chunks. Only the measurement
matters here: the production chunker (Rust, host side) must be deterministic, not
identical to this one, since a mismatch costs efficiency, never correctness (the
assembled file is still verified by sha256).
"""
import hashlib
import random
import sys

import os
AVG = int(os.environ.get("CHUNK_AVG_KIB", "1024")) * 1024  # average chunk; 1 MiB by default
MIN = AVG // 4
MAX = AVG * 4
MASK = AVG - 1  # cut when the low bits are zero: AVG must be a power of two
_rng = random.Random(0x5EED5EED)
GEAR = [_rng.getrandbits(64) for _ in range(256)]
WORD = (1 << 64) - 1


def chunks(data):
    """[(offset, length, sha256)] over data."""
    out = []
    n = len(data)
    i = 0
    while i < n:
        end = min(i + MAX, n)
        cut = end
        k = min(i + MIN, n)
        h = 0
        while k < end:
            h = ((h << 1) + GEAR[data[k]]) & WORD
            if h & MASK == 0:
                cut = k + 1
                break
            k += 1
        out.append((i, cut - i, hashlib.sha256(data[i:cut]).hexdigest()))
        i = cut
    return out


def main(old_path, new_path):
    with open(old_path, "rb") as f:
        old = f.read()
    with open(new_path, "rb") as f:
        new = f.read()
    old_chunks = chunks(old)
    new_chunks = chunks(new)
    have = {h for _, _, h in old_chunks}
    reused = sum(length for _, length, h in new_chunks if h in have)
    fetched = len(new) - reused
    print(f"old: {len(old)} bytes in {len(old_chunks)} chunks")
    print(f"new: {len(new)} bytes in {len(new_chunks)} chunks")
    print(f"reused from old: {reused} bytes ({100 * reused / len(new):.1f}%)")
    print(f"to fetch: {fetched} bytes ({fetched / 1048576:.1f} MB)")
    # Machine-readable last line for the report.
    print(f"delta reused_pct={100 * reused / len(new):.1f} fetch_mb={fetched / 1048576:.1f}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
