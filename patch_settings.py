import sys

with open('src/settings.rs', 'r', encoding='utf-8') as file:
    content = file.read()
    
content = content.replace('"v1.0.0"', '"v1.0.1"')

with open('src/settings.rs', 'w', encoding='utf-8') as file:
    file.write(content)

