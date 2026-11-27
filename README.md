<img src="assets/icon.svg" width="72" height="72" alt="Logo de photosort">

# photosort

Aplicación de escritorio para **catalogar una biblioteca personal de fotos y vídeos sin tocarla**. Lee las carpetas que elijas, analiza cada archivo en tu equipo y guarda etiquetas, obras y personas en su propia base de datos. Solo exporta copias cuando tú lo pides.

Escrita en Rust con [egui](https://github.com/emilk/egui). Windows 10 y 11 de 64 bits.

Consulta la **[guía de uso](docs/uso.md)** para empezar paso a paso.

## Qué hace

- **Catálogo y duplicados**: indexado incremental por contenido (SHA-256) con detección de duplicados exactos y de imágenes parecidas a partir de vectores visuales.
- **Vídeos y formatos modernos**: cataloga vídeos (`.mp4`, `.webm`, `.3gp`, `.mkv`, `.wmv`, `.mov`, `.avi`) mediante Media Foundation de Windows, e imágenes HEIC, HEIF, AVIF, JXL y JFIF mediante WIC, además de JPEG, PNG, WebP, GIF, BMP y TIFF.
- **Lectura de texto (OCR)**: lee texto en español, inglés, japonés, chino y coreano, y permite buscar tanto en ese texto como en las rutas.
- **Reconocimiento visual con SigLIP 2**: clasifica el tipo de imagen y categorías de contenido independientes, con umbrales calibrados sobre una muestra real de 335 imágenes para los tipos, `Anime` y `Portada de manga` (ver [docs/vision-evaluacion.md](docs/vision-evaluacion.md#calibración-con-imágenes-reales)). Permite crear reglas propias y buscar por descripción.
- **Personas y rostros**: detecta rostros con YuNet y, si se activa «Agrupar rostros» con SFace, agrupa caras claras de fotos para que el usuario les ponga nombre y una grupos.
- **Obras de manga**: distingue a qué obra, capítulo y página **pertenece** una imagen frente a las obras que **menciona** en su texto leído; reconoce tipos de página (reclutamiento, anuncio, créditos, promocional, portada, capítulo), propone grupos por carpeta y detecta portadas en páginas promocionales.
- **Subcategorías y etiquetas**: categorías en árbol, renombrado, traslado, fusión y borrado del catálogo. Cada sugerencia se acepta o rechaza y un análisis posterior no pisa tu decisión.
- **Fechas, formatos y filtros**: filtra por tipo de medio, categorías con modos (exigir, incluir alguna, excluir), etiquetas, entidades, formato, orientación, año de captura y personas.
- **Revisión en segundo plano**: desactivada por defecto; procesa por lotes sin compartir VRAM entre visión y OCR, permite volver a revisar fotos concretas y guarda los fallos en una lista de errores para reintentarlos sin bucles. Ver [docs/revision.md](docs/revision.md).
- **Exportación verificada**: copias byte a byte verificadas con SHA-256, restauración de fechas, manifiestos CSV y JSON, sidecars XMP opcionales, incrustación opcional en copias JPEG y disposición «Conservar carpetas».
- **Accesibilidad**: navegación completa con teclado, foco visible, contraste comprobado y árbol de accesibilidad para Narrador de Windows.

## Tus originales y tu privacidad

- Los originales nunca se borran, mueven, renombran, sobrescriben ni se vuelven a codificar, y no se crean archivos junto a ellos.
- Toda escritura pasa por un protector de rutas que rechaza cualquier destino dentro de una biblioteca registrada. Al leer un archivo, Windows puede actualizar su fecha de último acceso según la configuración del equipo.
- Todo se procesa en tu equipo, sin servicios en línea, APIs de pago ni telemetría. El catálogo se guarda en `%LOCALAPPDATA%\photosort`.

## Compilar y ejecutar

Necesitas Windows 10 u 11 de 64 bits, [Rust](https://rustup.rs/) (`rust-toolchain.toml`) y las herramientas de compilación de C++ de Visual Studio. Una GPU con DirectX 12 acelera el análisis; también funciona en CPU. La primera compilación descarga las dependencias de Rust y ONNX Runtime.

```powershell
cargo run --release -p app
```

Para probar la interfaz con datos de demostración, sin usar tus fotos:

```powershell
cargo run --release -p app -- --demo
```

## Modelos

**El repositorio no incluye modelos y la aplicación no los descarga.** Cada archivo está fijado por tamaño y SHA-256. Colócalos en una carpeta fuera de tu biblioteca y selecciónala en **Configuración → Modelos y dispositivo** (ver [docs/uso.md](docs/uso.md#activar-los-modelos)).

| Uso | Modelos | Carpeta, archivos y huellas |
|---|---|---|
| Reconocer imágenes y rostros | SigLIP 2 base 256 y YuNet | [docs/vision-evaluacion.md](docs/vision-evaluacion.md) |
| Agrupar rostros por persona (opcional) | SFace (`sface/face_recognition_sface_2021dec.onnx`) | [docs/vision.md](docs/vision.md) |
| Leer texto (OCR) | PP-OCRv6 Small y reconocedor coreano PP-OCRv5 | [docs/ocr-evaluacion.md](docs/ocr-evaluacion.md) |

## Desarrollo y licencia

```powershell
git config core.hooksPath .githooks   # una vez por clon
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Consulta [CONTRIBUTING.md](CONTRIBUTING.md) para las reglas de contribución y las pruebas con modelos reales, y [docs/decisiones.md](docs/decisiones.md) y [docs/diseno.md](docs/diseno.md) para el diseño técnico.

El código de photosort se publica bajo la [licencia MIT](LICENSE). Las licencias de las dependencias, ONNX Runtime, `DirectML.dll` y los modelos se resumen en [docs/terceros.md](docs/terceros.md).
