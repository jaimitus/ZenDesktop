# 🧘 ZenDesktop

> Organizador dinámico de escritorio ultraligero y nativo para Windows.
> Alternativa moderna a Stardock Fences, gratuita y open-source.

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![Windows](https://img.shields.io/badge/platform-Windows%2010%2F11-blue.svg)]()
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/jaimitus/ZenDesktop)](https://github.com/jaimitus/ZenDesktop/releases)

<p align="center">
  <img src="assets/icons/preview.html" alt="ZenDesktop Preview" width="700">
</p>

## ✨ Características

- **🪟 Cajas flotantes translúcidas** — Agrupa archivos del escritorio en cajas elegantes con transparencia real (canal alfa)
- **🎯 Drag & Drop inteligente** — Arrastra archivos entre cajas, al escritorio, o dentro de subcarpetas con feedback visual (highlight + icono)
- **🤖 Organización automática** — Reglas por extensión, nombre, patrón o IA local (Ollama) para clasificar archivos automáticamente
- **📦 Drop desde Explorer** — Suelta archivos desde cualquier carpeta directamente en las cajas (OLE drag & drop nativo)
- **🔍 Búsqueda integrada** — Filtra archivos dentro de cada caja en tiempo real
- **📐 Grid + Lista** — Modo cuadrícula o lista, con ordenación por nombre, tamaño, tipo o fecha
- **🎨 Personalización total** — Colores, bordes, esquinas redondeadas, tipografía, iconos, contador de elementos
- **📂 Soporte multi-escritorio** — Escritorio público, OneDrive, y cualquier carpeta adicional
- **⏱️ Archivado automático** — Mueve archivos antiguos a carpeta de archivo con tiempo configurable
- **🌙 Modo Zen** — Oculta/muestra todas las cajas con doble clic en el escritorio o Ctrl+Alt+Z
- **🌍 Multi-idioma** — Español, Inglés, Alemán, Francés, Portugués, Italiano
- **🪶 Ultraligero** — ~780 KB, ~4 MB RAM en reposo, 0% CPU (100% dirigido por eventos, sin polling)
- **🔔 Toast notifications** — Feedback visual con iconos contextuales (🟢 drops, 🔵 organización)
- **🤖 IA local (Ollama)** — Clasificación semántica y creación automática de cajas con IA

## 📥 Instalación

### Portable (recomendado)
1. Descarga `ZenDesktop.exe` de [Releases](https://github.com/jaimitus/ZenDesktop/releases)
2. Ejecútalo — se minimiza a la bandeja del sistema
3. La configuración se guarda en `%APPDATA%\ZenDesktop\config.toml`

### MSI Installer
1. Descarga `ZenDesktop-1.0.0.msi` de [Releases](https://github.com/jaimitus/ZenDesktop/releases)
2. Instala normalmente — se añade al inicio de Windows
3. La configuración se guarda en `%APPDATA%\ZenDesktop\config.toml`

### Desde código
```bash
git clone https://github.com/jaimitus/ZenDesktop.git
cd ZenDesktop/src/code
cargo build --release
# El binario está en target/release/zendesktop.exe
```

## 🚀 Uso rápido

| Acción | Atajo / Gesto |
|---|---|
| Abrir configuración | Clic derecho en icono de bandeja → Settings |
| Modo Zen (mostrar/ocultar) | Doble clic en escritorio vacío o `Ctrl+Alt+Z` |
| Mover archivos entre cajas | Seleccionar → Arrastrar a otra caja |
| Soltar en subcarpeta | Arrastrar sobre una carpeta dentro de una caja |
| Devolver al escritorio | Arrastrar fuera de las cajas |
| Buscar en una caja | Clic en la barra de búsqueda 🔍 |
| Cambiar orden | Clic derecho → Sort by |
| Bloquear caja | Clic en el candado 🔒 |

## ⚙️ Configuración

La configuración se edita desde la ventana de Settings (accesible desde el icono de bandeja) o directamente en `config.toml`:

```toml
[general]
language = "es"
sweep_interval_minutes = 15

[appearance]
background = "#1A1B2E"
corner_radius = 12.0
font_family = "Segoe UI"

[[rules]]
id = "documentos"
folder = "Documentos"
extensions = ["pdf", "docx", "txt", "md"]
color = "#4CCD3C"
enabled = true
```

## 🏗️ Arquitectura

- **Renderizado**: Direct2D + DirectWrite sobre ventanas *layered* (canal alfa por píxel, 0% VRAM)
- **Vigilancia**: `ReadDirectoryChangesW` nativo vía `notify` — sin polling, 0% CPU en reposo
- **Drag & Drop**: Captura manual (`SetCapture`) + `WM_MOUSEMOVE` para arrastre entre cajas; `IDropTarget` OLE para drop desde Explorer
- **Idiomas**: Sistema de traducción estática con `Tr` struct — sin allocations en runtime
- **Binario**: ~780 KB sin comprimir (~420 KB con UPX), sin runtime externo

## 📁 Estructura del proyecto

```
src/code/
├── Cargo.toml          # Dependencias y perfil release
├── build.rs            # Compila recursos Windows (.rc)
├── assets/
│   └── zendesktop.rc   # Icono y metadatos del EXE
└── src/
    ├── main.rs         # Entry point + hilo de mensajes
    ├── config.rs       # Carga/guarda config.toml (serde)
    ├── rules.rs        # Motor de reglas y organización
    ├── ui.rs           # Ventanas, renderizado, drag & drop
    ├── settings.rs     # Ventana de configuración
    ├── watcher.rs      # Monitor de cambios en disco
    ├── ai.rs           # Cliente HTTP para Ollama
    └── i18n.rs         # Traducciones (6 idiomas)
```

## 🤝 Contribuir

1. Fork el repositorio
2. Crea una rama (`git checkout -b feature/nombre`)
3. Haz commit (`git commit -m "feat: descripcion"`)
4. Push (`git push origin feature/nombre`)
5. Abre un Pull Request

## 📄 Licencia

MIT © 2024-2026 ZenDesktop Core Team

---

Hecho con ❤️ y Rust. [Reportar bug](https://github.com/jaimitus/ZenDesktop/issues) · [Discusiones](https://github.com/jaimitus/ZenDesktop/discussions)
