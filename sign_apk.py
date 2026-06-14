#!/usr/bin/env python3
"""
Sign APK with debug keystore.
Usage: python3 sign_apk.py <apk_path>
"""
import os
import sys
import subprocess
import tempfile
import shutil

def sign_apk(apk_path):
    # Find apksigner
    build_tools = "/home/shiyi/Android/Sdk/build-tools/37.0.0"
    apksigner = os.path.join(build_tools, "apksigner")
    
    if not os.path.exists(apksigner):
        print(f"Error: apksigner not found at {apksigner}")
        return False
    
    # Debug keystore
    keystore = os.path.expanduser("~/.android/debug.keystore")
    if not os.path.exists(keystore):
        print(f"Error: debug keystore not found at {keystore}")
        return False
    
    print(f"Signing: {apk_path}")
    
    # Sign with debug keystore
    cmd = [
        apksigner, "sign",
        "--ks", keystore,
        "--ks-pass", "pass:android",
        "--key-pass", "pass:android",
        "--in", apk_path,
        "--out", apk_path + ".signed"
    ]
    
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Error signing APK: {result.stderr}")
        return False
    
    # Replace original with signed
    shutil.move(apk_path + ".signed", apk_path)
    print(f"Signed: {apk_path}")
    return True

if __name__ == '__main__':
    apk_path = sys.argv[1] if len(sys.argv) > 1 else None
    if not apk_path:
        print("Usage: python3 sign_apk.py <apk_path>")
        sys.exit(1)
    
    if not os.path.exists(apk_path):
        print(f"Error: APK not found: {apk_path}")
        sys.exit(1)
    
    success = sign_apk(apk_path)
    sys.exit(0 if success else 1)
