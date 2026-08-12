import sys

files = [
    'assets/zendesktop.rc',
    'installer/zendesktop.iss',
    'scripts/build-release.ps1',
    'src/settings.rs',
    'README.md'
]

for f in files:
    with open(f, 'rb') as file:
        content = file.read()
    
    # decode, replace, encode
    has_bom = content.startswith(b'\xef\xbb\xbf')
    if has_bom:
        content_str = content[3:].decode('utf-8')
    else:
        content_str = content.decode('utf-8')
        
    content_str = content_str.replace('1.0.0', '1.0.1')
    
    with open(f, 'wb') as file:
        if has_bom:
            file.write(b'\xef\xbb\xbf')
        file.write(content_str.encode('utf-8'))
        
    print(f"Updated {f}")

