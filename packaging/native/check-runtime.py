#!/usr/bin/env python3
"""Reject release binaries that rely on build-machine media/CRT installations."""
import pathlib
import platform
import re
import struct
import subprocess
import sys


WINDOWS_SYSTEM_DLLS = frozenset(
    "advapi32 avrt bcrypt bcryptprimitives cabinet cfgmgr32 combase crypt32 cryptbase "
    "cryptnet d3d11 d3d12 dcomp dnsapi dwmapi dxgi dxva2 gdi32 hid imagehlp imm32 "
    "iphlpapi kernel32 kernelbase mf mfplat mfreadwrite mfuuid mmdevapi mpr mswsock "
    "ncrypt netapi32 normaliz ntdll ole32 oleaut32 opengl32 powrprof profapi propsys "
    "psapi rpcrt4 sechost secur32 setupapi shell32 shlwapi sspicli ucrtbase user32 "
    "userenv usp10 uuid version winhttp wininet winmm winspool wintrust wldap32 "
    "ws2_32 wtsapi32".split()
)


def pe_imports(binary):
    """Read normal and delay-load PE imports, without a runner-specific SDK tool."""
    data = pathlib.Path(binary).read_bytes()
    if data[:2] != b"MZ":
        raise ValueError("Expected a Windows PE executable")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\0\0":
        raise ValueError("Invalid PE signature")
    sections = struct.unpack_from("<H", data, pe + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe + 20)[0]
    optional = pe + 24
    magic = struct.unpack_from("<H", data, optional)[0]
    if magic != 0x20B:
        raise ValueError("Only 64-bit Windows release binaries are supported")
    directory = optional + 112
    section_table = optional + optional_size

    def offset(rva):
        for index in range(sections):
            base = section_table + index * 40
            virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
                "<IIII", data, base + 8
            )
            if virtual_address <= rva < virtual_address + max(virtual_size, raw_size):
                return raw_offset + rva - virtual_address
        raise ValueError(f"Unmapped PE RVA {rva:#x}")

    def cstring(rva):
        start = offset(rva)
        return data[start : data.index(b"\0", start)].decode("ascii").lower()

    imports = []
    for directory_index, entry_size, name_index in ((1, 20, 3), (13, 32, 1)):
        rva, size = struct.unpack_from("<II", data, directory + directory_index * 8)
        if not rva:
            continue
        start = offset(rva)
        for entry in range(start, start + size, entry_size):
            fields = struct.unpack_from("<" + "I" * (entry_size // 4), data, entry)
            if not any(fields):
                break
            if directory_index == 13 and fields[0] != 1:
                raise ValueError("Expected RVA-based delay imports")
            imports.append(cstring(fields[name_index]))
    if not imports:
        raise ValueError("No Windows import table found")
    return sorted(set(imports))


def check(binary):
    system = platform.system()
    if system == "Windows":
        dependencies = pe_imports(binary)
        invalid = [
            dep for dep in dependencies
            if not (
                dep.endswith(".dll")
                and (dep[:-4] in WINDOWS_SYSTEM_DLLS or dep.startswith(("api-ms-win-", "ext-ms-win-")))
            )
        ]
    elif system == "Darwin":
        output = subprocess.check_output(["otool", "-L", str(binary)], text=True)
        dependencies = [line.strip().split(" (", 1)[0] for line in output.splitlines()[1:]]
        invalid = [dep for dep in dependencies if not dep.startswith(("/usr/lib/", "/System/Library/"))]
    elif system == "Linux":
        # readelf does not execute the candidate binary, unlike ldd.
        output = subprocess.check_output(["readelf", "-d", str(binary)], text=True)
        dependencies = re.findall(r"\(NEEDED\).*\[(.*?)\]", output)
        permitted = {"libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1", "libgcc_s.so.1", "ld-linux-x86-64.so.2"}
        invalid = [dep for dep in dependencies if dep not in permitted]
    else:
        raise ValueError(f"Unsupported release platform: {system}")
    if invalid:
        raise ValueError("Non-system runtime dependencies: " + ", ".join(invalid))
    print("Verified system-only runtime dependencies: " + ", ".join(dependencies))
    subprocess.run([str(pathlib.Path(binary).resolve()), "media-worker", "--check"], check=True)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: check-runtime.py PATH_TO_XCOC")
    check(pathlib.Path(sys.argv[1]))
