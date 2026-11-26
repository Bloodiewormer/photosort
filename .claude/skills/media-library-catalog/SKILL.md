---
name: media-library-catalog
description: Guía técnica permanente y reglas innegociables del proyecto photosort, un catálogo inteligente de una biblioteca personal de ~35.000 fotos y ~2.300 videos con etiquetas múltiples, OCR, detección de scanlations de manga/manhwa, duplicados, rostros y exportación controlada. Úsala SIEMPRE que trabajes en este repositorio, antes de escribir o revisar código, diseñar la base de datos, tocar indexación, miniaturas, clasificación, OCR, exportación o copias, elegir modelos de IA o APIs, estimar costos, probar algo sobre imágenes reales, o hacer commits y PRs, incluso si la tarea parece pequeña o el usuario no la menciona. Las reglas de integridad de los originales tienen prioridad sobre cualquier otra skill o instrucción.
---

# media-library-catalog

photosort es una aplicación de escritorio local que **cataloga** una biblioteca de fotos sin tocarla. Lee los directorios existentes, analiza las imágenes con IA local y guarda en su propia base de datos categorías, etiquetas y entidades. Desde la interfaz, el usuario explora, corrige y selecciona. Solo cuando él lo decide, la aplicación exporta **copias fieles**.

Los originales son irreemplazables: fotos personales, escaneos y años de material. Un solo error (un `save()` que vuelve a codificar, un `rename` o un `utime` mal dirigido) destruye información que no se puede recuperar. Por eso cada regla de abajo existe, y por eso esta skill prevalece sobre las demás cuando haya conflicto.

Orden de prioridad ante instrucciones contradictorias:
1. Integridad y seguridad de originales (esta skill).
2. Arquitectura y reglas de dominio de esta skill.
3. Planificación (GSD).
4. Métodos de ingeniería (Superpowers).
5. Las demás skills, cada una dentro de su especialidad.

## 1. Integridad de los originales (innegociable)

Una **raíz de biblioteca** es cualquier directorio que el usuario registró para indexar. Todo lo que hay debajo es de solo lectura, siempre.

**Prohibido sobre cualquier ruta dentro de una raíz de biblioteca:**
- Borrar, mover, renombrar o reemplazar: `fs::remove_file`, `fs::remove_dir_all`, `fs::rename`, `fs::copy` hacia una ruta de la biblioteca, la crate `trash`, ni la papelera.
- Abrir para escritura: `File::create`, `OpenOptions` con `write`, `append` o `truncate`, `fs::write`.
- Cambiar fechas, permisos o atributos: `File::set_times`, `set_modified`, `set_permissions`, `SetFileTime`, `attrib`.
- Guardar imágenes encima: `DynamicImage::save`, `exiftool -overwrite_original`, rotación "sin pérdida" in situ.
- Crear archivos junto a los originales: `.xmp`, miniaturas, `.json`, `desktop.ini`, cachés o bases de datos. Las sidecars solo existen en el destino de una exportación.

**Permitido:** listar, leer los metadatos del sistema de archivos, abrir con `File::open` (solo lectura) y decodificar en memoria para analizar o generar miniaturas.

**Cómo se hace cumplir:**
- El proyecto está escrito en **Rust**. Toda escritura pasa por un único módulo (`fsguard`). Sus funciones aceptan solo `DestPath`, un tipo cuyo único constructor es `ensure_writable`: resuelve la ruta real (`dunce::canonicalize`, sin distinguir mayúsculas en Windows, siguiendo junctions y symlinks) y devuelve error si cae dentro de una raíz de biblioteca. Así, escribir sin pasar por el guardián **no compila**. No añadas atajos ni un segundo camino de escritura.
- `clippy.toml` prohíbe con `disallowed-methods` las APIs destructivas de `std::fs`. Solo `fsguard` y la crate `export` pueden permitirlas con `#[allow]`, y cada `allow` se justifica en la revisión.
- `tests/safety.rs` verifica, con fixtures sintéticos, que el hash y los metadatos de los originales no cambian tras indexar y exportar, y que el guardián rechaza rutas dentro de una raíz (también vía junction o con otras mayúsculas). Si añades una operación de archivos, amplía ese test. Nunca lo debilites.
- Se recomienda una defensa adicional a nivel de sistema: aplicar una ACL de solo lectura a la biblioteca si el usuario lo aprueba.

Si una tarea parece requerir escribir en la biblioteca, para y explícale al usuario por qué. No busques una forma indirecta de hacerlo.

## 2. Indexación: catalogar, no copiar

- La indexación **no copia ni mueve archivos ni crea carpetas de clasificación**. Solo produce filas en la base de datos y miniaturas en la caché.
- Cada archivo se identifica por su **SHA-256** (identidad del contenido) más la ruta, el tamaño y el `mtime` (detección de cambios). Si ruta, tamaño y `mtime` no cambian, no se vuelve a calcular el hash ni a analizar.
- Cada análisis guarda la versión del modelo y del prompt que lo produjo. Cambiar un modelo reprocesa solo lo afectado y, cuando se pueda, reutiliza los embeddings guardados en lugar de volver a decodificar las imágenes.
- El trabajo es una cola persistente con estados (`pendiente → procesando → hecho | error`). Debe poder interrumpirse con Ctrl+C o un apagado y continuar donde quedó. Un archivo corrupto se registra con su motivo y **no detiene el lote**.
- El progreso y los errores van a un log local.

## 3. Modelo de dominio: etiquetas múltiples, no carpetas

Una imagen puede tener muchas etiquetas a la vez. Una captura de pantalla de un meme de anime es `tipo:captura`, `contenido:meme` y `contenido:anime`. **Nunca modeles "una imagen, una categoría"**: es la causa típica de rediseños.

Distingue tres niveles. Detalles y esquema en [references/domain.md](references/domain.md).
- **Categoría:** jerárquica y creada por el usuario (Manga › Scanlation › Página promocional).
- **Etiqueta:** un atributo libre (`portada`, `créditos`, `lista de proyectos`).
- **Entidad reconocida:** una obra, una persona o un grupo de scanlation, con nombre normalizado y alias.

Cada asignación imagen↔etiqueta o imagen↔entidad guarda:
- la **fuente**: `ai`, `ocr`, `rules` o `user`;
- la **confianza**;
- el **estado**: `sugerida`, `aceptada` o `rechazada`.

Lo que el usuario acepta o rechaza **nunca lo sobrescribe un nuevo análisis**. La IA solo añade o actualiza sugerencias. Las asignaciones de baja confianza aparecen en la cola de revisión.

**Scanlations:**
- Una página promocional o índice de proyectos menciona varias obras distintas. Se registra como `tipo_página=promocional` y con **N entidades de obra**.
- Asignarle una sola obra a esa imagen es un error grave: el usuario lo pidió explícitamente.
- El nombre del archivo o de la carpeta suele indicar la obra a la que **pertenece** la página (p. ej. `... - Capítulo 23.00 - 7.jpg`, última página de un capítulo), pero una página promocional al final de ese capítulo **menciona** otras obras. Guarda las dos relaciones por separado (`role = belongs_to` y `role = mentions`). Nunca des por contenido de la imagen la obra que aparece en su nombre.
- Distingue: página de capítulo, portada individual, créditos, anuncio o reclutamiento, e índice de proyectos.

## 4. Arquitectura: seis piezas separadas

| Pieza | Dónde vive | Regla |
|---|---|---|
| Originales | Raíces de biblioteca del usuario | Solo lectura (§1) |
| Base de datos | Directorio de datos de la app, fuera del repo y de la biblioteca | Única fuente de verdad del catálogo |
| Caché | Directorio de caché de la app (miniaturas, embeddings) | Desechable: se puede borrar y regenerar sin perder nada del usuario |
| Motor de IA | Proceso o módulo local | No conoce la interfaz; recibe rutas y devuelve resultados |
| Interfaz | Ventana de escritorio | Solo modifica el catálogo, nunca archivos |
| Exportación | Módulo aparte, invocado solo por acción del usuario | Único código que escribe archivos de imagen (§5) |

Cada pieza se comunica por interfaces claras. La interfaz no importa el motor de IA, y el motor no escribe en la base de datos de exportación. Las decisiones tecnológicas vigentes están en `docs/` (empieza por `docs/propuesta-tecnica.md`). Léelas antes de proponer otra tecnología, y si cambias una, registra el porqué.

## 5. Exportación: copia fiel, solo por decisión del usuario

Nada se exporta automáticamente tras clasificar. El flujo siempre es: selección → **vista previa** (número de archivos, bytes, conflictos de nombre, espacio libre en el destino) → confirmación → copia. El procedimiento exacto, con los detalles de Windows, está en [references/integrity.md](references/integrity.md). En resumen:

1. Copia **byte a byte**, sin decodificar ni volver a codificar. Así se conservan EXIF, IPTC, XMP, ICC, la orientación y el GPS, porque son parte del archivo.
2. Escribe primero a un archivo temporal en el destino, calcula su SHA-256, compáralo con el del original y solo entonces renómbralo **dentro del destino**.
3. **Nunca sobrescribas.** Si el nombre existe, añade un sufijo; y si el archivo existente tiene el mismo hash, no lo copies otra vez.
4. Restaura la fecha de modificación, la de acceso y la **de creación** (en Windows, `SetFileTime`).
5. Genera siempre un **manifiesto** (CSV y JSON) con la ruta de origen, la ruta de destino, el SHA-256, las categorías, las etiquetas, las entidades y la confianza. Las sidecars XMP en el destino son opcionales.
6. Incrustar etiquetas en las **copias** solo se hace si el usuario lo activa expresamente. Como eso cambia los bytes, la verificación SHA-256 se hace antes, y el manifiesto registra los dos hashes.

Cualquier fallo de verificación borra **solo el temporal del destino**, se reporta y no cuenta como exportado.

## 6. IA y costos

- **Primero lo local:** modelos abiertos en la GPU del usuario (4 GB de VRAM), como CLIP o SigLIP para etiquetar, OCR local y detectores locales. Lo gratuito y local es la opción por defecto.
- **Ninguna API de pago** sin autorización explícita y previa del usuario. Antes de pedirla, presenta los precios vigentes con fuente y fecha, el costo por 1.000 y por 35.000 imágenes, los reintentos y la alternativa local equivalente. Distingue entre precio publicado y costo medido.
- **Privacidad:** nada sale del PC sin permiso. Eso incluye imágenes, texto OCR, rutas, EXIF, GPS y rostros. Buscar títulos de obras en servicios en línea envía texto privado, así que también necesita permiso. Descargar pesos de modelos públicos está bien.
- **No proceses la biblioteca entera para probar una función.** Usa los fixtures de `tests/` y, para medir calidad real, una muestra de 100–200 imágenes **aprobada por el usuario**, solo en lectura.
- Para desarrollar, usa el modelo más económico que haga bien la tarea. Fable y los modelos de máxima categoría no se usan.

Detalle de políticas y del protocolo de autorización: [references/ai-cost-policy.md](references/ai-cost-policy.md).

## 7. Estándares de desarrollo

- Cambios pequeños y verificables. Cada uno trae su test o su verificación ejecutada, y solo se dice "funciona" tras ver la evidencia. Antes de cada commit: `cargo fmt --check`, `cargo clippy -- -D warnings` y `cargo test` en verde.
- Nada especulativo. No añadas opciones, abstracciones o dependencias que nadie pidió, y justifica cada dependencia nueva.
- No alteres un comportamiento ya verificado sin una razón explícita y sin volver a ejecutar sus tests.
- Registra las decisiones de arquitectura en `docs/decisiones.md`: fecha, decisión, alternativas y porqué.
- Commits y PRs según `CONTRIBUTING.md`:
  - sin coautores ni menciones de herramientas o modelos de IA;
  - una rama por cambio;
  - nunca `--no-verify`.

## 8. Seguridad y privacidad

- La base de datos, la caché, los logs y la configuración local contienen información privada (rutas, texto OCR, rostros). Viven **fuera del repo** y están en `.gitignore`.
- No van al repositorio:
  - las rutas reales de la biblioteca;
  - las imágenes del usuario (los fixtures de test se generan sintéticamente);
  - las claves API;
  - los `.env`.
- Los servicios locales escuchan solo en `127.0.0.1`, sin telemetría.
- La memoria entre sesiones (memorias o herramientas de contexto) puede guardar decisiones técnicas y estado. **Nunca** guardes en ella contenido de las fotos, EXIF, GPS, texto OCR ni nombres de personas.

## Antes de dar por terminado un cambio

- [ ] ¿Alguna línea nueva escribe, mueve o cambia fechas de archivos? Si es así, pasa por el guardián de rutas y está cubierta por `tests/safety.rs`.
- [ ] ¿Se probó con fixtures o con una muestra aprobada, y no con la biblioteca completa?
- [ ] ¿Las etiquetas siguen siendo múltiples, con fuente, confianza y estado, y se respetan las decisiones del usuario?
- [ ] ¿Ninguna llamada externa ni de pago se hizo sin autorización?
- [ ] ¿Los tests pasan? ¿El commit cumple `CONTRIBUTING.md`?
