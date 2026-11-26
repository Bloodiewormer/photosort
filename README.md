<img src="assets/icon.svg" width="72" height="72" alt="Logo de photosort">

# photosort

Aplicación de escritorio para **catalogar una biblioteca personal de fotos sin tocarla**. Lee las carpetas que elijas, reconoce lo que hay en cada imagen en tu propio equipo y guarda las etiquetas en su base de datos. Solo exporta copias cuando tú lo pides.

Escrita en Rust con [egui](https://github.com/emilk/egui). Windows.

> En desarrollo. La calidad de la clasificación visual con fotos reales **todavía no está medida** y sus umbrales no están calibrados: trata las etiquetas como sugerencias.

## Qué hace

- **Catálogo** de miles de imágenes, con indexado incremental y duplicados exactos.
- **Etiquetas múltiples**: categorías, etiquetas y obras. Cada sugerencia se acepta o se rechaza, y un nuevo análisis no pisa tu decisión.
- **Lectura de texto (OCR)** en español, inglés, japonés, chino y coreano, y búsqueda por ese texto.
- **Obras de manga** mencionadas en páginas promocionales, a partir de tu propia lista.
- **Reconocimiento visual**: manga, ilustración, foto, captura, meme, documento, presencia de personas y etiquetas que describas con tus palabras.
- **Exportación** con copias byte a byte, verificadas con SHA-256, con sus fechas y un manifiesto CSV y JSON.

Lo pendiente está en las [issues](https://github.com/Bloodiewormer/photosort/issues).

## Tus originales

- Nunca se borran, mueven, renombran, sobrescriben ni se vuelven a codificar.
- No se crean archivos junto a ellos.
- El código solo puede escribir a través de un protector de rutas que rechaza cualquier destino dentro de tu biblioteca, y las pruebas comparan los originales antes y después.
- Una salvedad: al **leer** un archivo, Windows puede actualizar su fecha de último acceso, según la configuración del equipo.

## Privacidad

Todo se procesa en tu equipo. No hay servicios en línea, APIs de pago ni telemetría en el análisis. El catálogo, que incluye el texto leído de tus imágenes, se guarda en `%LOCALAPPDATA%\photosort`: trátalo como información privada.

## Requisitos

- Windows 10 u 11 de 64 bits.
- [Rust](https://rustup.rs/) (la versión la fija `rust-toolchain.toml`) y las herramientas de compilación de C++ de Visual Studio.
- Para el análisis con modelos, una GPU compatible con DirectX 12 es opcional; también funciona en CPU, más despacio. Probado en un equipo con una GPU de 4 GB.

La primera compilación descarga las dependencias de Rust y ONNX Runtime.

## Compilar y ejecutar

```powershell
cargo run --release -p app
```

Para probarla sin ninguna foto tuya, con datos inventados:

```powershell
cargo run --release -p app -- --demo
```

## Modelos

**El repositorio no incluye modelos y la aplicación no los descarga.** Cada archivo está fijado por tamaño y SHA-256, y la app se niega a cargar uno distinto.

| Para | Modelos | Origen, archivos y huellas |
|---|---|---|
| Leer texto | PP-OCRv6 Small y reconocedor coreano PP-OCRv5 | [docs/ocr-evaluacion.md](docs/ocr-evaluacion.md) |
| Reconocer imágenes | SigLIP 2 base 256 y YuNet | [docs/vision-evaluacion.md](docs/vision-evaluacion.md) |

Cada modelo tiene su propia licencia, indicada en esos documentos.

## Desarrollo

```powershell
git config core.hooksPath .githooks   # una vez por clon
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Las pruebas usan solo datos sintéticos. Las que necesitan modelos quedan como **omitidas** y GitHub no las ejecuta; las instrucciones para ejecutarlas están en [CONTRIBUTING.md](CONTRIBUTING.md). GitHub ejecuta el resto en cada PR. Antes de contribuir, lee [CONTRIBUTING.md](CONTRIBUTING.md).

| Carpeta | Contenido |
|---|---|
| `crates/fsguard` | Protector de rutas |
| `crates/scanner`, `crates/catalog` | Escaneo en solo lectura y catálogo SQLite |
| `crates/thumbs`, `crates/rules` | Miniaturas y reglas sin modelos |
| `crates/ocr`, `crates/vision` | Lectura de texto y reconocimiento visual |
| `crates/export` | Copias verificadas y manifiesto |
| `crates/app` | Interfaz |
| `docs/` | Propuesta, [decisiones](docs/decisiones.md) y [diseño](docs/diseno.md) |

## Licencia

El código de photosort se publica con la [licencia MIT](LICENSE). Las bibliotecas, `DirectML.dll`, ONNX Runtime y los modelos conservan sus propias licencias; antes de distribuir un instalador hay que revisarlas ([issue](https://github.com/Bloodiewormer/photosort/issues/42)).
