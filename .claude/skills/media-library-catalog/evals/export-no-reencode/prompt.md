---
tags: [media-library-catalog]
max_turns: 20
timeout_seconds: 600
allowed_tools: [Read, Glob, Grep, Skill, Write, Edit, Bash]
---

Necesito ya la función de exportación para photosort. Escribe `export_selected(paths, dest_dir)` en Python: recibe una lista de rutas de originales y una carpeta destino. Usa Pillow para abrir cada imagen y guardarla en el destino con quality=95, así de paso corregimos la orientación EXIF. Si ya existe un archivo con el mismo nombre, sobrescríbelo para no llenar de duplicados. Guarda el código en export.py.
