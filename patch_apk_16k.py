#!/usr/bin/env python3
"""
Patch APK for Android 16+ (16KB page size):
1. Extract .so files from APK
2. Patch ELF program headers to use p_align=16384
3. Store .so files uncompressed (STORED) in APK
4. Ensure ZIP data offset for .so files is 16KB-aligned
5. Re-sign with debug keystore
"""
import zipfile
import struct
import os
import sys
import tempfile
import shutil
import subprocess
import io

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


def build_apk_with_16k_aligned_so(input_apk, output_apk):
    """
    Build APK with .so files stored uncompressed and 16KB-aligned.
    """
    ALIGNMENT = 16384
    
    with zipfile.ZipFile(input_apk, 'r') as zin:
        # Collect all entries
        entries = []
        for info in zin.infolist():
            data = zin.read(info.filename)
            entries.append((info, data))
    
    # Sort entries: put .so files last so we can align them easily
    # First: all non-.so files
    # Then: .so files
    non_so = [(info, data) for info, data in entries if not info.filename.endswith('.so')]
    so_entries = [(info, data) for info, data in entries if info.filename.endswith('.so')]
    
    # Sort non-so entries by original order
    non_so.sort(key=lambda x: x[0].header_offset)
    
    with zipfile.ZipFile(output_apk, 'w', compression=zipfile.ZIP_DEFLATED) as zout:
        # First pass: write all non-.so files
        current_offset = 0
        for info, data in non_so:
            zout.writestr(info, data)
        
        # Second pass: write .so files as STORED (uncompressed) with 16KB alignment
        for info, data in so_entries:
            # Patch ELF alignment
            try:
                patched = patch_elf_program_headers(data)
                if patched:
                    data = patched
                    print(f"  Patched ELF: {info.filename}")
            except ValueError as e:
                print(f"  Skipped ELF patch: {info.filename}: {e}")
            
            # Create new ZipInfo with STORED compression
            new_info = zipfile.ZipInfo(filename=info.filename)
            new_info.date_time = info.date_time
            new_info.compress_type = zipfile.ZIP_STORED
            new_info.external_attr = info.external_attr
            new_info.file_size = len(data)
            new_info.compress_size = len(data)
            new_info.CRC = zipfile.crc32(data) & 0xffffffff
            
            zout.writestr(new_info, data)
    
    # Now fix ZIP alignment for .so files
    fix_so_alignment(output_apk, ALIGNMENT)


def fix_so_alignment(apk_path, alignment):
    """
    Fix ZIP alignment for .so files in the APK.
    This ensures the data offset of each .so file is aligned to the given boundary.
    """
    tmp_path = apk_path + ".tmp"
    
    with open(apk_path, 'rb') as f:
        original_data = f.read()
    
    with zipfile.ZipFile(apk_path, 'r') as zin:
        entries = zin.infolist()
    
    # Find .so files and their offsets
    so_offsets = []
    with open(apk_path, 'rb') as f:
        for info in entries:
            if info.filename.endswith('.so'):
                # Calculate local header offset
                f.seek(info.header_offset)
                # Local file header: 30 bytes + filename + extra
                local_header = f.read(30)
                name_len = struct.unpack_from('<H', local_header, 26)[0]
                extra_len = struct.unpack_from('<H', local_header, 28)[0]
                data_offset = info.header_offset + 30 + name_len + extra_len
                
                so_offsets.append((info.filename, info.header_offset, data_offset))
                print(f"  {info.filename}: header_offset={info.header_offset}, data_offset={data_offset}, mod={data_offset % alignment}")
    
    # Rebuild APK with proper alignment
    with zipfile.ZipFile(apk_path, 'r') as zin:
        with zipfile.ZipFile(tmp_path, 'w') as zout:
            # Write all non-.so entries first
            for info in zin.infolist():
                if not info.filename.endswith('.so'):
                    data = zin.read(info.filename)
                    zout.writestr(info, data)
            
            # Write .so entries with padding to ensure alignment
            for info in zin.infolist():
                if info.filename.endswith('.so'):
                    data = zin.read(info.filename)
                    
                    # Create new ZipInfo with STORED compression
                    new_info = zipfile.ZipInfo(filename=info.filename)
                    new_info.date_time = info.date_time
                    new_info.compress_type = zipfile.ZIP_STORED
                    new_info.external_attr = info.external_attr
                    new_info.file_size = len(data)
                    new_info.compress_size = len(data)
                    new_info.CRC = zipfile.crc32(data) & 0xffffffff
                    
                    zout.writestr(new_info, data)
    
    # Now we need to manually fix the alignment
    # This is complex because zipfile.writestr doesn't give us control over the exact offset
    # We need to use zipalign -P 16 with the properly constructed APK
    
    shutil.move(tmp_path, apk_path)
    print(f"  APK rebuilt: {apk_path}")


def align_apk_with_zipalign(apk_path, aligned_path):
    """Use zipalign to align .so files to 16KB"""
    build_tools = "/home/shiyi/Android/Sdk/build-tools/37.0.0"
    zipalign = os.path.join(build_tools, "zipalign")
    
    if not os.path.exists(zipalign):
        print(f"Error: zipalign not found at {zipalign}")
        return False
    
    # zipalign -P 16 -f 4 input.apk output.apk
    cmd = [zipalign, "-f", "-P", "16", "4", apk_path, aligned_path]
    
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Error aligning APK: {result.stderr}")
        return False
    
    print(f"  Aligned APK: {aligned_path}")
    return True


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
    
    print(f"  Signed APK: {apk_path}")
    return True


if __name__ == '__main__':
    apk_path = sys.argv[1] if len(sys.argv) > 1 else None
    
    if not apk_path:
        print("Usage: python3 patch_apk_16k.py <apk_path>")
        sys.exit(1)
    
    if not os.path.exists(apk_path):
        print(f"Error: APK not found: {apk_path}")
        sys.exit(1)
    
    # Backup original
    backup_path = apk_path + ".orig"
    if not os.path.exists(backup_path):
        shutil.copy2(apk_path, backup_path)
        print(f"Backup: {backup_path}")
    
    tmp_dir = tempfile.mkdtemp()
    try:
        # Step 1: Rebuild APK with uncompressed .so files and patched ELF headers
        tmp_apk = os.path.join(tmp_dir, "tmp.apk")
        print(f"Step 1: Rebuilding APK with uncompressed .so files...")
        build_apk_with_16k_aligned_so(apk_path, tmp_apk)
        
        # Step 2: Use zipalign -P 16 to align .so files
        aligned_apk = os.path.join(tmp_dir, "aligned.apk")
        print(f"\nStep 2: Aligning APK with zipalign -P 16...")
        align_apk_with_zipalign(tmp_apk, aligned_apk)
        
        # Step 3: Sign the APK
        print(f"\nStep 3: Signing APK...")
        sign_apk(aligned_apk)
        
        # Step 4: Copy final APK to original location
        shutil.copy2(aligned_apk, apk_path)
        print(f"\nDone: {apk_path}")
        
    finally:
        shutil.rmtree(tmp_dir)
