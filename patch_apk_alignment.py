#!/usr/bin/env python3
"""
Patch ELF .so files inside an APK to use 16KB page alignment for Android 16+
Preserves APK structure, then re-signs with debug keystore.
"""
import zipfile
import struct
import os
import sys
import tempfile
import shutil
import subprocess

def patch_elf_program_headers(data):
    """Patch ELF64 program headers to use 16KB alignment"""
    result = bytearray(data)
    
    if result[:4] != b'\x7fELF':
        raise ValueError("Not an ELF file")
    
    ei_class = result[4]
    ei_data = result[5]
    
    if ei_class != 2:
        raise ValueError("Only ELF64 supported")
    
    endian = '<' if ei_data == 1 else '>'
    
    e_phoff = struct.unpack_from(endian + 'Q', result, 32)[0]
    e_phentsize = struct.unpack_from(endian + 'H', result, 54)[0]
    e_phnum = struct.unpack_from(endian + 'H', result, 56)[0]
    
    if e_phentsize != 56:
        raise ValueError(f"Unexpected program header size: {e_phentsize}")
    
    p_align_offset = 48
    
    modified = False
    for i in range(e_phnum):
        ph_offset = e_phoff + i * e_phentsize
        align_field_offset = ph_offset + p_align_offset
        
        old_align = struct.unpack_from(endian + 'Q', result, align_field_offset)[0]
        
        if old_align > 0 and old_align < 16384:
            struct.pack_into(endian + 'Q', result, align_field_offset, 16384)
            modified = True
    
    return bytes(result) if modified else None

def patch_apk(apk_path):
    tmp_dir = tempfile.mkdtemp()
    try:
        tmp_apk = os.path.join(tmp_dir, os.path.basename(apk_path))
        
        with zipfile.ZipFile(apk_path, 'r') as zin:
            with zipfile.ZipFile(tmp_apk, 'w', zipfile.ZIP_DEFLATED) as zout:
                for item in zin.infolist():
                    data = zin.read(item.filename)
                    
                    if item.filename.endswith('.so'):
                        try:
                            patched = patch_elf_program_headers(data)
                            if patched:
                                data = patched
                                print(f"  Patched: {item.filename}")
                        except ValueError as e:
                            print(f"  Skipped {item.filename}: {e}")
                    
                    zout.writestr(item, data)
        
        shutil.move(tmp_apk, apk_path)
        print(f"Patched APK: {apk_path}")
        
    finally:
        shutil.rmtree(tmp_dir)

def sign_apk(apk_path):
    build_tools = "/home/shiyi/Android/Sdk/build-tools/37.0.0"
    apksigner = os.path.join(build_tools, "apksigner")
    keystore = os.path.expanduser("~/.android/debug.keystore")
    
    if not os.path.exists(apksigner):
        print(f"Error: apksigner not found at {apksigner}")
        return False
    if not os.path.exists(keystore):
        print(f"Error: debug keystore not found at {keystore}")
        return False
    
    cmd = [
        apksigner, "sign",
        "--ks", keystore,
        "--ks-pass", "pass:android",
        "--key-pass", "pass:android",
        apk_path
    ]
    
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Error signing APK: {result.stderr}")
        return False
    
    print(f"Signed APK: {apk_path}")
    return True

if __name__ == '__main__':
    apk_path = sys.argv[1] if len(sys.argv) > 1 else None
    
    if not apk_path:
        print("Usage: python3 patch_apk_alignment.py <apk_path>")
        sys.exit(1)
    
    if not os.path.exists(apk_path):
        print(f"Error: APK not found: {apk_path}")
        sys.exit(1)
    
    # Backup original
    backup_path = apk_path + ".orig"
    if not os.path.exists(backup_path):
        shutil.copy2(apk_path, backup_path)
        print(f"Backup: {backup_path}")
    
    print(f"Patching: {apk_path}")
    patch_apk(apk_path)
    
    sign_apk(apk_path)
    
    print("Done.")
