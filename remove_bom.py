import os

files = [
    'assets/zendesktop.rc',
    'installer/zendesktop.iss',
    'scripts/build-release.ps1',
    'src/settings.rs',
    'README.md',
    'CHANGELOG.md'
]

for f in files:
    with open(f, 'rb') as file:
        content = file.read()
    if content.startswith(b'\xef\xbb\xbf'):
        content = content[3:]
        with open(f, 'wb') as file:
            file.write(content)
        print(f"Removed BOM from {f}")

