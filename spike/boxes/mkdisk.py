#!/usr/bin/env python3 -I
"""Pad a script to a 512-byte multiple so libkrun accepts it as a raw disk."""
import sys
src, dst = sys.argv[1], sys.argv[2]
data = open(src, 'rb').read()
pad = (-len(data)) % 512
open(dst, 'wb').write(data + b'\n' * pad)
