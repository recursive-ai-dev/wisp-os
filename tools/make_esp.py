#!/usr/bin/env python3
"""Build the ISO's deterministic, mount-free FAT16 EFI boot image.

Only EFI/BOOT/BOOTX64.EFI and APPS/*.wisp are admitted. The image is a
16 MiB FAT16 superfloppy, as required by an EFI El Torito boot entry;
it is not a partitioned installation image and must not be flashed to disks.
"""

import argparse
import os
from pathlib import Path
import re
import stat
import struct
import tempfile

SECTOR = 512
TOTAL_SECTORS = 32768
FAT_SECTORS = 128
ROOT_ENTRIES = 512
ROOT_SECTORS = ROOT_ENTRIES * 32 // SECTOR
ROOT_START = 1 + 2 * FAT_SECTORS
DATA_START = ROOT_START + ROOT_SECTORS
CLUSTERS = TOTAL_SECTORS - DATA_START


class Node:
    def __init__(self, name, data=None):
        self.name = name
        self.data = data
        self.children = []
        self.cluster = 0
        self.clusters = 0
        self.alias = b""


def _checksum(alias):
    value = 0
    for byte in alias:
        value = (((value & 1) << 7) + (value >> 1) + byte) & 255
    return value


def _alias(name, used):
    base, dot, ext = name.rpartition(".")
    if not dot:
        base, ext = name, ""
    plain = re.fullmatch(r"[A-Za-z0-9_]{1,8}", base) and re.fullmatch(r"[A-Za-z0-9_]{0,3}", ext)
    candidate = (base.upper().ljust(8) + ext.upper().ljust(3)).encode("ascii") if plain else None
    if candidate is not None and candidate not in used:
        used.add(candidate)
        return candidate
    cleaned = re.sub(r"[^A-Z0-9_]", "_", base.upper()) or "FILE"
    suffix = re.sub(r"[^A-Z0-9_]", "_", ext.upper())[:3]
    for number in range(1, 10000):
        tail = f"~{number}"
        candidate = (cleaned[:8 - len(tail)] + tail).ljust(8).encode("ascii") + suffix.ljust(3).encode("ascii")
        if candidate not in used:
            used.add(candidate)
            return candidate
    raise ValueError("too many colliding FAT filenames")


def _long_entries(name, alias):
    units = list(struct.unpack("<" + "H" * (len(name.encode("utf-16le")) // 2), name.encode("utf-16le")))
    if not units or len(units) > 255:
        raise ValueError("FAT filename must contain 1 to 255 UTF-16 units")
    # A full last entry needs no terminator. Otherwise pad after its NUL.
    if len(units) % 13:
        units.append(0)
        units.extend([0xFFFF] * (-len(units) % 13))
    count = len(units) // 13
    result = bytearray()
    positions = (1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30)
    for index in range(count, 0, -1):
        entry = bytearray(32)
        entry[0] = index | (0x40 if index == count else 0)
        entry[11] = 0x0F
        entry[13] = _checksum(alias)
        for position, unit in zip(positions, units[(index - 1) * 13:index * 13]):
            struct.pack_into("<H", entry, position, unit)
        result.extend(entry)
    return result


def _entry(alias, attr, cluster, size=0):
    entry = bytearray(32)
    entry[:11] = alias
    entry[11] = attr
    struct.pack_into("<H", entry, 16, 33)  # 1980-01-01; deterministic FAT dates.
    struct.pack_into("<H", entry, 18, 33)
    struct.pack_into("<H", entry, 24, 33)
    struct.pack_into("<H", entry, 26, cluster)
    struct.pack_into("<I", entry, 28, size)
    return entry


def _source_tree(source):
    root = Node("")
    efi = Node("EFI")
    boot = Node("BOOT")
    apps = Node("APPS")
    binary = source / "EFI" / "BOOT" / "BOOTX64.EFI"
    if not binary.is_file() or binary.is_symlink():
        raise ValueError(f"missing regular EFI binary: {binary}; run ./build.sh first")
    boot.children.append(Node("BOOTX64.EFI", binary.read_bytes()))
    efi.children.append(boot)
    root.children.extend((efi, apps))
    app_dir = source / "APPS"
    if app_dir.is_dir():
        names = set()
        for path in sorted(app_dir.iterdir(), key=lambda item: item.name.casefold()):
            if path.suffix.casefold() != ".wisp":
                continue
            if not path.is_file() or path.is_symlink():
                raise ValueError(f"app must be a regular file: {path}")
            if len(path.name.encode("utf-8")) > 64 or len(apps.children) >= 64:
                raise ValueError("apps exceed WISP's filename/count snapshot limits")
            if re.search(r'[\x00-\x1f\\/:*?"<>|]', path.name) or path.name.endswith((" ", ".")):
                raise ValueError(f"invalid FAT filename: {path.name}")
            if path.name.casefold() in names:
                raise ValueError(f"case-insensitive duplicate app name: {path.name}")
            names.add(path.name.casefold())
            data = path.read_bytes()
            if len(data) > 256 * 1024:
                raise ValueError(f"app exceeds WISP's 256 KiB source limit: {path.name}")
            data.decode("utf-8")
            apps.children.append(Node(path.name, data))
    if sum(len(node.name.encode("utf-8")) + len(node.data) for node in apps.children) > 1024 * 1024:
        raise ValueError("apps exceed WISP's 1 MiB total snapshot limit")
    return root


def build_image(source):
    root = _source_tree(Path(source))
    nodes = []

    def prepare(directory):
        used = set()
        for child in directory.children:
            child.alias = _alias(child.name, used)
            nodes.append(child)
            if child.data is None:
                prepare(child)
        # '.' and '..', one end marker, and long/short entries for children.
        size = 96 + sum(len(_long_entries(child.name, child.alias)) + 32 for child in directory.children)
        directory.clusters = max(1, (size + SECTOR - 1) // SECTOR)

    prepare(root)
    next_cluster = 2
    for node in nodes:
        if node.data is not None:
            node.clusters = (len(node.data) + SECTOR - 1) // SECTOR
        if node.clusters:
            node.cluster = next_cluster
            next_cluster += node.clusters
    if next_cluster - 2 > CLUSTERS:
        raise ValueError("EFI binary and apps do not fit the 16 MiB boot image")

    image = bytearray(TOTAL_SECTORS * SECTOR)
    image[:3] = b"\xeb\x3c\x90"
    image[3:11] = b"WISPOS  "
    struct.pack_into("<HBHBHHBHHHII", image, 11,
                     SECTOR, 1, 1, 2, ROOT_ENTRIES, TOTAL_SECTORS, 0xF8,
                     FAT_SECTORS, 63, 255, 0, 0)
    image[36] = 0x80
    image[38] = 0x29
    image[39:43] = b"WISP"
    image[43:54] = b"WISP LIVE  "
    image[54:62] = b"FAT16   "
    image[510:512] = b"\x55\xaa"
    fat = bytearray(FAT_SECTORS * SECTOR)
    struct.pack_into("<HH", fat, 0, 0xFFF8, 0xFFFF)
    for node in nodes:
        for offset in range(node.clusters):
            cluster = node.cluster + offset
            struct.pack_into("<H", fat, cluster * 2,
                             cluster + 1 if offset + 1 < node.clusters else 0xFFFF)
        if node.data is not None and node.data:
            start = (DATA_START + node.cluster - 2) * SECTOR
            image[start:start + len(node.data)] = node.data
    image[SECTOR:(1 + FAT_SECTORS) * SECTOR] = fat
    image[(1 + FAT_SECTORS) * SECTOR:ROOT_START * SECTOR] = fat

    def write_directory(directory, parent):
        entries = bytearray()
        if directory is root:
            entries.extend(_entry(b"WISP LIVE  ", 0x08, 0))
        else:
            entries.extend(_entry(b".          ", 0x10, directory.cluster))
            entries.extend(_entry(b"..         ", 0x10, parent.cluster))
        for child in directory.children:
            entries.extend(_long_entries(child.name, child.alias))
            entries.extend(_entry(child.alias, 0x10 if child.data is None else 0x20,
                                  child.cluster, 0 if child.data is None else len(child.data)))
            if child.data is None:
                write_directory(child, directory)
        entries.extend(bytes(32))
        start = ROOT_START * SECTOR if directory is root else (DATA_START + directory.cluster - 2) * SECTOR
        image[start:start + len(entries)] = entries

    write_directory(root, root)
    return image


def write_image(source, destination):
    destination = Path(destination)
    if destination.exists() or destination.is_symlink():
        if not stat.S_ISREG(destination.lstat().st_mode):
            raise ValueError("output must be a regular file, never a device or symlink")
        print(f"Replacing boot image: {destination}")
    image = build_image(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=destination.parent, prefix=".wisp-esp-", delete=False) as output:
            temporary = output.name
            output.write(image)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, destination)
        temporary = None
    finally:
        if temporary:
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="ESP source directory (usually ./esp)")
    parser.add_argument("output", type=Path, help="regular FAT image output file")
    args = parser.parse_args()
    try:
        write_image(args.source, args.output)
    except (OSError, ValueError, UnicodeError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
