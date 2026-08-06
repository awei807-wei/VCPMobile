#!/usr/bin/env python3
"""校验真机 APK 的 ABI、ELF 16KB 页对齐与未压缩库 ZIP 对齐。"""

from __future__ import annotations

import argparse
import struct
import sys
import zipfile
from pathlib import Path


PAGE_SIZE = 16 * 1024
SUPPORTED_ABIS = {"arm64-v8a", "armeabi-v7a", "x86", "x86_64"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path, help="待校验的 APK 路径")
    parser.add_argument(
        "--expected-abi",
        default="arm64-v8a",
        choices=sorted(SUPPORTED_ABIS),
        help="APK 必须包含且默认只能包含的 ABI",
    )
    parser.add_argument(
        "--allow-extra-abis",
        action="store_true",
        help="允许 universal APK 同时携带其他 ABI",
    )
    return parser.parse_args()


def read_elf_load_alignments(payload: bytes, name: str) -> list[int]:
    if len(payload) < 64 or payload[:4] != b"\x7fELF":
        raise ValueError(f"{name}: 不是有效 ELF 文件")

    elf_class = payload[4]
    data_encoding = payload[5]
    if data_encoding == 1:
        endian = "<"
    elif data_encoding == 2:
        endian = ">"
    else:
        raise ValueError(f"{name}: 未知 ELF 字节序 {data_encoding}")

    if elf_class == 2:
        program_offset = struct.unpack_from(endian + "Q", payload, 32)[0]
        entry_size = struct.unpack_from(endian + "H", payload, 54)[0]
        entry_count = struct.unpack_from(endian + "H", payload, 56)[0]
        minimum_entry_size = 56
        alignment_offset = 48
        alignment_format = "Q"
    elif elf_class == 1:
        program_offset = struct.unpack_from(endian + "I", payload, 28)[0]
        entry_size = struct.unpack_from(endian + "H", payload, 42)[0]
        entry_count = struct.unpack_from(endian + "H", payload, 44)[0]
        minimum_entry_size = 32
        alignment_offset = 28
        alignment_format = "I"
    else:
        raise ValueError(f"{name}: 未知 ELF class {elf_class}")

    if entry_size < minimum_entry_size:
        raise ValueError(f"{name}: ELF program header 尺寸异常 {entry_size}")

    alignments: list[int] = []
    for index in range(entry_count):
        entry_offset = program_offset + index * entry_size
        entry_end = entry_offset + entry_size
        if entry_end > len(payload):
            raise ValueError(f"{name}: ELF program header 越界")

        program_type = struct.unpack_from(endian + "I", payload, entry_offset)[0]
        if program_type != 1:  # PT_LOAD
            continue
        alignments.append(
            struct.unpack_from(
                endian + alignment_format,
                payload,
                entry_offset + alignment_offset,
            )[0]
        )

    if not alignments:
        raise ValueError(f"{name}: 未找到 PT_LOAD 段")
    return alignments


def zip_data_offset(apk_file, info: zipfile.ZipInfo) -> int:
    apk_file.seek(info.header_offset)
    header = apk_file.read(30)
    if len(header) != 30 or header[:4] != b"PK\x03\x04":
        raise ValueError(f"{info.filename}: ZIP local header 无效")
    file_name_length, extra_length = struct.unpack_from("<HH", header, 26)
    return info.header_offset + 30 + file_name_length + extra_length


def verify_apk(apk_path: Path, expected_abi: str, allow_extra_abis: bool) -> None:
    if not apk_path.is_file():
        raise ValueError(f"APK 不存在: {apk_path}")

    errors: list[str] = []
    checked_libraries = 0
    stored_libraries = 0

    with apk_path.open("rb") as raw_apk, zipfile.ZipFile(raw_apk) as archive:
        native_entries = [
            info
            for info in archive.infolist()
            if info.filename.startswith("lib/") and info.filename.endswith(".so")
        ]
        if not native_entries:
            errors.append("APK 中没有原生 .so 库")

        packaged_abis = {
            info.filename.split("/", 2)[1]
            for info in native_entries
            if len(info.filename.split("/", 2)) == 3
        }
        if expected_abi not in packaged_abis:
            errors.append(
                f"缺少目标 ABI {expected_abi}；实际 ABI: {', '.join(sorted(packaged_abis)) or '无'}"
            )

        unexpected_abis = packaged_abis - {expected_abi}
        if unexpected_abis and not allow_extra_abis:
            errors.append(
                "包含非目标 ABI: " + ", ".join(sorted(unexpected_abis))
            )

        for info in native_entries:
            checked_libraries += 1
            try:
                alignments = read_elf_load_alignments(archive.read(info), info.filename)
                too_small = [value for value in alignments if value < PAGE_SIZE]
                if too_small:
                    errors.append(
                        f"{info.filename}: PT_LOAD p_align 小于 16KB: "
                        + ", ".join(str(value) for value in too_small)
                    )

                if info.compress_type == zipfile.ZIP_STORED:
                    stored_libraries += 1
                    offset = zip_data_offset(raw_apk, info)
                    if offset % PAGE_SIZE != 0:
                        errors.append(
                            f"{info.filename}: ZIP 数据偏移 {offset} 未按 16KB 对齐"
                        )
            except (OSError, struct.error, ValueError, zipfile.BadZipFile) as error:
                errors.append(str(error))

    if errors:
        details = "\n".join(f"- {error}" for error in errors)
        raise ValueError(f"真机 APK 校验失败:\n{details}")

    print(f"APK: {apk_path}")
    print(f"ABI: {expected_abi}")
    print(f"原生库: {checked_libraries} 个")
    print(f"未压缩且已校验 ZIP 16KB 对齐的库: {stored_libraries} 个")
    print("结果: ABI 与 ELF 16KB 页对齐校验通过")


def main() -> int:
    args = parse_args()
    try:
        verify_apk(args.apk, args.expected_abi, args.allow_extra_abis)
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
