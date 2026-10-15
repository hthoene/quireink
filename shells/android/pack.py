"""Adds the native library, the assets and the code to the APK aapt2 made,
the same way on every computer: in sorted order, stored (not compressed,
so no zlib version can change a byte; Android also runs the library
straight from the APK then), with a fixed time.

Usage: python pack.py APK DIR...   (each DIR's files go in under their
paths relative to DIR)
"""
import os
import sys
import zipfile

# The time aapt2 gives its own entries.
TIME = (1981, 1, 1, 1, 1, 2)


def main():
    apk, dirs = sys.argv[1], sys.argv[2:]
    files = []
    for top in dirs:
        for root, _, names in os.walk(top):
            for name in names:
                path = os.path.join(root, name)
                files.append((os.path.relpath(path, top).replace(os.sep, "/"), path))
    with zipfile.ZipFile(apk, "a") as zf:
        for name, path in sorted(files):
            info = zipfile.ZipInfo(name, TIME)
            info.compress_type = zipfile.ZIP_STORED
            info.create_system = 0
            info.external_attr = 0
            with open(path, "rb") as f:
                zf.writestr(info, f.read())


if __name__ == "__main__":
    main()
