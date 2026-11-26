# Modo demo y capturas de interfaz

`photosort` incluye un modo demo sintético y una opción de captura automática para revisar la interfaz visualmente sin tocar ninguna foto personal.

## Ejecutar el modo demo

```powershell
cargo run -p app -- --demo
```

Opciones útiles:

- `--demo-count <N>`: número de imágenes sintéticas a generar (por defecto `160`, máximo `2000`).
- `--data-dir <carpeta>`: carpeta base para guardar el entorno demo (`<carpeta>\library` y `<carpeta>\data`). Si se omite, se crea un directorio temporal que se elimina al cerrar.
- `--theme dark|light`: fuerza el tema oscuro o claro durante la ejecución.
- `--window-size <Ancho>x<Alto>`: tamaño inicial de la ventana en píxeles (por defecto `1280x820`).
- `--view gallery|review|duplicates|export`: selecciona la vista inicial y el criterio de espera al tomar capturas (por defecto `gallery`).

## Tomar una captura de pantalla

```powershell
cargo run -p app -- --demo --screenshot "$env:TEMP\photosort-gallery-dark.png" --view gallery --theme dark
cargo run -p app -- --demo --screenshot "$env:TEMP\photosort-review-light.png" --view review --theme light
cargo run -p app -- --demo --screenshot "$env:TEMP\photosort-duplicates-dark.png" --view duplicates --theme dark
cargo run -p app -- --demo --screenshot "$env:TEMP\photosort-export-light.png" --view export --theme light
```

Cómo funciona `--screenshot <salida.png>` según `--view`:

- **`gallery`** (por defecto): selecciona automáticamente la primera imagen para poblar el inspector y espera a que carguen las miniaturas visibles y la vista previa.
- **`review`**: abre la pestaña **Revisión** y espera a que terminen de cargar las miniaturas del grupo activo.
- **`duplicates`**: abre la pestaña **Duplicados** y espera a que carguen las miniaturas de todos los grupos visibles.
- **`export`**: abre el asistente de exportación sobre todas las imágenes demo con destino preconfigurado en `<base demo>\export-out` (fuera de la biblioteca demo y validado con `fsguard::ensure_export_dir`), calcula la vista previa y espera a que esté lista (`Ready`) junto con las miniaturas de fondo.

En todos los casos:

1. Tras cumplirse el criterio de carga de la vista, deja pasar unos fotogramas de estabilización (`--screenshot-delay-frames <N>`, por defecto `3`), solicita la captura al viewport de `egui`, guarda el archivo PNG y cierra la aplicación con código `0` (o distinto de cero si hay error o tiempo de espera agotado).
2. La ruta de salida se valida con `fsguard::ensure_writable` frente a las raíces de biblioteca registradas: cualquier ruta dentro de la biblioteca demo es rechazada.
3. Si se indica un nombre de vista desconocido en `--view`, el programa termina con error y un mensaje explicativo en español.

## Qué contienen los datos demo

Todo se genera por código con una semilla fija y reproducible:

- **Subcarpetas**: `capturas`, `documentos`, `manga`, `memes` y `personas`.
- **Imágenes sintéticas** (PNG, JPEG y GIF válidos): cuadradas, 16:9, 9:16 de móvil, tiras verticales 1:3 tipo webtoon, tamaños muy pequeños, variantes PNG con canal alfa y varios duplicados exactos byte a byte entre subcarpetas.
- **Patrones distintos**: degradados, franjas, damero y bloques de color con un número identificador dibujado con una fuente de mapa de bits integrada (sin archivos de fuentes externos).
- **Nombres que activan el motor de reglas (`rules::run`)**: capturas (`Screenshot_2024-01-02.png`), resoluciones de pantalla sin EXIF, archivos de WhatsApp (`IMG-20240101-WA0001.jpg`), GIF y tiras verticales largas.
- **Etiquetas sembradas en el catálogo**: categorías (`Memes`, `Anime`, `Manga`, `Documentos`, `Capturas de pantalla`, `Personas`), etiquetas y entidades ficticias (`Crónicas del Relojero`, `Academia Nebulosa`, etc.), con fuentes `rules`, `ai`, `ocr` y `user`, confianzas entre `0.22` y `0.95`, y mezcla de estados aceptados, sugeridos y rechazados.

## Garantía de aislamiento

El modo demo **no lee ni escribe nada real**:

- No accede a unidades ni carpetas personales del usuario.
- Toda la biblioteca sintética, la configuración (`settings.json`), la base de datos (`catalog.sqlite`) y la caché de miniaturas viven exclusivamente dentro del directorio temporal o del `--data-dir` indicado.
- Una vez registrada la carpeta `library` como biblioteca, todas las escrituras (catálogo, miniaturas y capturas) pasan por `fsguard::DestPath`, garantizando que la biblioteca permanece estrictamente en modo lectura.
