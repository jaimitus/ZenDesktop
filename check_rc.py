import sys
with open('assets/zendesktop.rc', 'rb') as f:
    b = f.read(20)
    print("Bytes:", b.hex())
