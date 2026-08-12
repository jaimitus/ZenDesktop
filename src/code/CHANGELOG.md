# Changelog

Todas las versiones notables de ZenDesktop están documentadas aquí.

El formato sigue [Keep a Changelog](https://keepachangelog.com/es/1.0.0/),
y el versionado sigue [SemVer](https://semver.org/lang/es/).

---

## [1.0.0] - 2026-08-12

### 🎉 Primera versión estable

Tras meses de desarrollo, ZenDesktop 1.0.0 está listo para producción.

### ✨ Funcionalidades

- **Cajas flotantes translúcidas** con renderizado Direct2D + DirectWrite
- **Drag & Drop manual** entre cajas, subcarpetas y al escritorio
- **Highlight visual** al arrastrar sobre subcarpetas destino
- **Drop desde Explorer** vía OLE `IDropTarget` (RegisterDragDrop)
- **Organización automática** por reglas (extensión, nombre, patrón)
- **Clasificación con IA** vía Ollama (local)
- **Modo Zen** con doble clic en escritorio o `Ctrl+Alt+Z`
- **6 idiomas**: Español, Inglés, Alemán, Francés, Portugués, Italiano
- **Toast notifications** con icono contextual (🟢 verde drops, 🔵 azul organize)
- **Búsqueda integrada** en cada caja
- **Modo cuadrícula** y lista
- **Ordenación** por nombre, tamaño, tipo, fecha o personalizada
- **Temas** personalizables: colores, bordes, tipografía, iconos
- **Archivado automático** por antigüedad
- **Soporte multi-escritorio** (OneDrive, público, etc.)
- **Bandera del sistema** con menú contextual
- **Binario ultraligero**: ~780 KB, ~4 MB RAM en reposo

### 🔧 Mejoras

- Toast con fuente GDI `DrawTextW` para soporte Unicode completo
- Ancho del toast calculado dinámicamente con `GetTextExtentPoint32W`
- Icono de checkmark verde en toast de drops
- Código muerto eliminado (COM drag, bitmap font, imports)
- 0 warnings de compilación

### 🐛 Correcciones

- Crash al hacer drag solucionado (drag manual sin COM)
- Archivos devueltos al escritorio no se reorganizan automáticamente
- Toast vacío solucionado (ahora usa GDI DrawTextW)
- Cursor cambia a manito + icono de archivo durante el arrastre
- Código de auto-organize no revierte drops manuales al escritorio

---

[1.0.0]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.0
