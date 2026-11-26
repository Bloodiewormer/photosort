# Propuesta técnica v3: catálogo inteligente de fotos (aplicación de escritorio nativa en Rust)

Fecha: 2026-11-26 · Estado: **propuesta, pendiente de aprobación.** No se ha leído, procesado ni exportado ningún archivo de la biblioteca.
Sustituye a la v1 (Immich y carpetas) y a la v2 (Python y web). Esta v3 cambia la pila a **Rust nativo** por decisión del usuario (2026-11-26). Las reglas permanentes del proyecto están en `.claude/skills/media-library-catalog/`.

## 0. Resumen

- **Decisión: aplicación propia y pequeña que reutiliza componentes abiertos existentes.**
  - Ningún gestor existente cubre a la vez lo que pides: etiquetas múltiples con confianza y revisión, entidades (obras) extraídas de páginas de scanlation, búsqueda OCR y exportación con vista previa, manifiesto y verificación SHA-256.
  - No se reinventa ningún modelo: todos los de IA son abiertos y locales.
- **Coste de funcionamiento: 0 €.** Todo se procesa en tu GPU (GTX 1650 Super, 4 GB). La alternativa de pago más barata costaría entre 5 y 38 USD por pasada para 35.000 fotos, y tus fotos saldrían del PC (§8).
- **Los originales nunca se tocan.** La indexación solo lee. Las etiquetas viven en la base de datos. Exportar es un acto explícito tuyo, con copias byte a byte verificadas.

## 1. Re-evaluación de herramientas existentes

| Herramienta | Qué cubre | Qué falta para este proyecto |
|---|---|---|
| **Immich 3.4** (web, Docker) | Búsqueda CLIP, OCR con búsqueda, rostros con nombres, duplicados, solo lectura `:ro` | No es de escritorio. Sin etiquetado IA automático ni etiquetas sugeridas con confianza. Sin entidades por imagen. Sin exportación con manifiesto. Extenderlo supone un fork de un proyecto grande en TS/Svelte. |
| **digiKam 8** (escritorio) | Galería, etiquetas jerárquicas, filtros, rostros, duplicados, auto-etiquetado de objetos | Sin OCR, sin categorías zero-shot propias, sin scanlations. Puede escribir metadatos y **borrar** archivos: incumple el requisito de no tener capacidad de eliminar. |
| PhotoPrism / LibrePhotos | Galería web, etiquetas fijas, rostros | Las mismas carencias que Immich, y menos activas. |
| Scripts CLIP (clipsort, zero-shot-image-classifier) | Clasificación zero-shot | Una sola etiqueta por imagen. Mueven archivos. Sin interfaz. |
| Google Vision, Rekognition, Gemini | Alta precisión | De pago por imagen y con envío de fotos privadas (§8). |

Conclusión: se reutilizan **modelos y librerías** (SigLIP, WD Tagger, RapidOCR, InsightFace, imagehash, exiftool), no un gestor completo.

## 2. Arquitectura (seis piezas separadas)

```
┌──────────────┐  solo "rb"   ┌──────────────────────── photosort ────────────────────────┐
│  ORIGINALES  │ ───────────► │ Indexador ──► Cola de trabajos (SQLite, reanudable)       │
│  (solo       │              │      │                     │                              │
│   lectura)   │              │      ▼                     ▼                              │
└──────────────┘              │  Caché  ◄── Motor IA (GPU): SigLIP · WD Tagger · OCR ·    │
                              │  miniaturas,   pHash · (rostros, después)                 │
                              │  embeddings        │                                      │
                              │                    ▼                                      │
                              │            Base de datos (SQLite + FTS5)                  │
                              │                    ▲                                      │
                              │   Interfaz (ventana de escritorio) ── solo edita catálogo │
                              │                    │  "Exportar…" (acción del usuario)    │
                              │                    ▼                                      │
                              │   Módulo de exportación ──► copias verificadas + manifiesto │
                              └───────────────────────────────────────────────────────────┘
```

| Pieza | Tecnología | Por qué |
|---|---|---|
| Originales | Tus carpetas actuales en D: | No se mueven. El guardián de rutas impide escribir en ellas. |
| Base de datos | **SQLite** (`rusqlite` con FTS5 incluido, WAL) para el texto OCR y los nombres | Un archivo local, sin servidor, rápido para 35.000 filas y con búsqueda de texto integrada. |
| Caché | Carpeta de datos de la app (`D:\photosort\cache\`) con miniaturas WebP de 256 px y 1024 px y embeddings | Desechable y regenerable. Unos 6 GB estimados. |
| Motor de IA | **Rust** + `ort` (ONNX Runtime con CUDA), `tokenizers` y `oar-ocr`, en el mismo proceso | Sin Python ni procesos auxiliares. Los modelos se usan en formato ONNX. |
| Interfaz | **egui/eframe** (nativa, MIT o Apache-2.0), con una galería que solo dibuja las filas visibles | La usan visores reales como Oculante. 35.000 miniaturas fluyen si se cargan como texturas bajo demanda. |
| Exportación | Crate Rust aislada (`export`) | Es el único código que escribe archivos de imagen. La fecha de creación en Windows se fija con la biblioteca estándar. |

Alternativas descartadas: **Java** (Swing con FlatLaf y un proceso Python para la IA; o Java puro, cuyas librerías de OCR están sin mantenimiento desde 2023-2024 y solo van en CPU) y **Slint** (licencia triple con obligaciones de atribución).

## 3. Modelos locales (todos gratuitos; caben en 4 GB de VRAM, de uno en uno)

| Tarea | Modelo | Notas |
|---|---|---|
| Etiquetas generales (multi-etiqueta) | **SigLIP 2** (base o so400m, fp16) | Usa una **sigmoide por etiqueta** en lugar de un softmax, así que cada etiqueta se puntúa por separado y una imagen puede ser "captura" y "meme" a la vez. |
| Anime, manga, manhwa, ilustración | **WD Tagger v3** (ONNX, Apache-2.0) | Etiquetas finas como `monochrome`, `comic`, `speech_bubble` o `screentone`. Solo se aplica a imágenes de tipo dibujo. |
| OCR | **oar-ocr** (PP-OCRv5 en ONNX con CUDA: latín, japonés, coreano, chino) | Se aplica solo a imágenes con texto probable: capturas, documentos, memes y manga. |
| Manga frente a manhwa | Señales combinadas: idioma OCR (kana o hangul), proporción vertical tipo webtoon, color y dirección de lectura | Es justo donde CLIP solo falla. |
| Páginas promocionales de scanlation | SigLIP con prompts específicos + detección de rectángulos tipo portada (OpenCV) + OCR + coincidencia difusa (rapidfuzz) con alias de obras | Una fila por **cada** obra detectada. Si la confianza es baja, va a revisión. |
| Casos difíciles (opcional) | VLM local pequeño (Qwen-VL de 2–3B cuantizado, llama.cpp) | Solo para las pocas imágenes en revisión. Lento, pero gratuito. |
| Duplicados | SHA-256 (exactos) + pHash/imagehash y similitud de embeddings (variantes) | Solo agrupa para revisar. No borra nada. |
| Rostros (después) | InsightFace (detección + embeddings) + clustering | Los pesos de InsightFace son para uso no comercial, lo que vale para un uso personal. Alternativa con licencia Apache: YuNet + SFace de OpenCV. |

### Riesgos conocidos de la pila Rust

- `ort` sigue en *release candidate* (2.0.0-rc.15) y `oar-ocr` es un proyecto joven con pocos usuarios. Mitigación: versiones fijadas y ejecución en CPU como respaldo.
- La GPU tiene 4 GB: los modelos se cargan de uno en uno y el codificador de texto de SigLIP corre en CPU.
- SigLIP exige escribir el texto en minúsculas y rellenar a 64 tokens; si no, los resultados se degradan. Hay un test para ello.
- Leer metadatos EXIF, IPTC, XMP e ICC requiere combinar varias crates. Siempre en solo lectura.
- Usar la GPU requiere instalar CUDA y cuDNN (la versión exacta se verifica al llegar a esa fase).
- El primer prototipo es más lento de construir que en Java o Python. Es el precio de aprender Rust.

## 4. Base de datos

Boceto completo en `.claude/skills/media-library-catalog/references/domain.md`. Claves:
- `asset` se identifica por SHA-256, y `location` guarda sus rutas: una imagen puede tener varias rutas, que son duplicados exactos.
- `category` es un árbol editable, `tag` es plana y `entity` (obra, persona, grupo) tiene alias.
- Las tablas de unión `asset_category`, `asset_tag` y `asset_entity` guardan **fuente** (`ai`/`ocr`/`rules`/`user`), **confianza** y **estado** (`sugerida`/`aceptada`/`rechazada`).
- Lo que tú aceptas o rechazas nunca lo sobrescribe un nuevo análisis.
- `job` es la cola reanudable, `analysis_run` registra la versión del modelo, y `export`/`export_item` el historial de exportaciones.

## 5. Flujo de procesamiento (incremental y reanudable)

1. **Escaneo:** recorre las carpetas elegidas y registra ruta, tamaño y `mtime`. Si un archivo no cambió, se omite.
2. **Hash y metadatos:** SHA-256, dimensiones, formato y EXIF/IPTC/XMP (leídos con exiftool, sin escribir).
3. **Miniaturas:** se generan en la caché, respetando la orientación EXIF en memoria.
4. **Embeddings SigLIP** y etiquetas zero-shot.
5. **WD Tagger** sobre las imágenes de tipo dibujo.
6. **OCR** sobre las imágenes con texto, indexado en FTS5.
7. **Scanlations:** tipo de página y entidades de obra.
8. **Duplicados:** agrupación.

Cada etapa es un trabajo con estado propio. Se puede pausar, apagar el PC y continuar. Los errores se registran y no detienen el lote. Si cambia un modelo, solo se repite su etapa.

**Rendimiento estimado** (sin medir; lo medirá el piloto): SigLIP y WD Tagger, decenas de imágenes por segundo; OCR, de 2 a 5 por segundo sobre el subconjunto con texto. Primera indexación completa: **del orden de 2 a 4 horas**. Las siguientes son incrementales y tardan minutos.

## 6. Interfaz

- **Barra lateral:** árbol de categorías, etiquetas, obras y carpetas originales, con contadores.
- **Galería virtualizada** con selección múltiple (clic, Mayús, Ctrl o "seleccionar todo el filtro").
- **Barra de búsqueda:** nombre de archivo, texto OCR y filtros combinados (Y/O/NO) por categorías, etiquetas, obras, fechas, tamaño y formato, con ordenamiento.
- **Visor individual:** la imagen, la ruta original, los metadatos y las etiquetas con su fuente y confianza. Desde ahí se aceptan, rechazan o añaden etiquetas.
- **Cola de revisión:** las sugerencias de baja confianza, con atajos de teclado para aceptar o rechazar rápido.
- **Duplicados:** grupos de exactos y de variantes, uno al lado del otro.
- **Asistente de exportación** (§7).

El diseño visual (paleta, tipografía, espaciado, atajos) se define antes de implementar. Unslop queda en espera porque su herramienta audita interfaces web.

## 7. Exportación (solo por decisión tuya)

1. Seleccionas imágenes una a una, por categoría o por una combinación de filtros.
2. Eliges el destino y la organización: estructura plana, por categoría, por obra, por fecha o la estructura original.
3. **Vista previa:** cuántos archivos, cuánto espacio, conflictos de nombre y espacio libre en el destino.
4. La copia se hace byte a byte a un temporal; se verifica el SHA-256 contra el original y después se renombra en el destino. Nunca se sobrescribe: si el nombre existe, se añade un sufijo, y si es idéntico, se omite.
5. Se conservan EXIF, IPTC, XMP, ICC, orientación y GPS, porque son parte del archivo, y también las fechas de modificación y de creación.
6. Siempre se genera un **manifiesto CSV y JSON** con las etiquetas de cada archivo. Las sidecars XMP en el destino son opcionales. Incrustar etiquetas en las copias es opcional y está desactivado por defecto; si se activa, se registran ambos hashes.

Procedimiento exacto: `.claude/skills/media-library-catalog/references/integrity.md`.

## 8. Costos

### 8.1 Desarrollo (asistentes de programación)
- Con Claude y Gemini, el gasto sale de tus suscripciones. No hay cargos aparte salvo que uses una clave de API.
- Política aplicada:
  - Gemini (Antigravity) para las tareas delegadas: investigación, calificación y revisiones;
  - Sonnet o Haiku cuando haga falta un agente de Claude;
  - **nunca Fable**.
- Esta conversación ya corre en **Sonnet 5.5**, más económico que Opus. Opus se reserva para decisiones de arquitectura difíciles.
- Ejemplo medido: cada prueba de la skill en Sonnet consumió unos 50–60 mil tokens.

### 8.2 Inferencia (la app clasificando fotos)

| Opción | Precio publicado (consultado el 2026-11-26) | 1.000 imágenes | 35.000 imágenes | Datos que salen del PC |
|---|---|---|---|---|
| **Local (propuesta)** | 0 | 0 | **0** (electricidad < 0,10 USD) | Ninguno |
| Gemini 2.5 Flash-Lite | 0,10 / 0,40 USD por 1M de tokens de entrada / salida | 0,16–0,23 | **5,5–8,2** | Las fotos |
| Gemini 3.5 Flash-Lite | 0,30 / 2,50 | 0,86–1,09 | **30–38** | Las fotos |
| Google Cloud Vision | 1,50 por 1.000 unidades y función (1.000 gratis al mes) | 0 | **51 por función** (etiquetas + OCR + rostros = 153) | Las fotos |
| AWS Rekognition | 1,00 por 1.000 | 1 | **35** | Las fotos |

- Son precios **publicados**, no medidos. Suponen una llamada por imagen, 100 tokens de prompt y 300 de salida, con imágenes de 384 px o de 1024 px.
- Cada reprocesamiento repite el costo; añade un 10–20 % por reintentos.
- El nivel gratuito de Gemini puede cubrir volúmenes pequeños, pero sus límites no están verificados, y las condiciones de uso de datos del nivel gratuito son distintas a las del de pago.
- Fuentes: aistudio.google.com/docs/pricing, cloud.google.com/vision/pricing, aws.amazon.com/rekognition/pricing.

**Recomendación:** procesamiento 100 % local. Si en el piloto alguna categoría no alcanza la calidad necesaria, se puede considerar un modelo externo **solo para las imágenes en revisión** (decenas o cientos, no 35.000), con un piloto de 100–200 imágenes, una estimación previa y tu autorización explícita.

### 8.3 Infraestructura
- Disco (estimado): en `D:\photosort\`, unos 6 GB de caché, menos de 1 GB de base de datos y 3–5 GB de modelos; en C:, unos 5–8 GB de herramientas (Rust, compilador de C++ y, para la GPU, CUDA y cuDNN).
- RAM: 14 GB es suficiente.
- No hay servidores ni suscripciones.

### 8.4 Mantenimiento
- 0 €. Solo hay que actualizar dependencias de forma controlada, con las versiones fijadas y los tests como red de seguridad.

## 9. Plan de entrega

1. **Prototipo (etapa 3):**
   - elegir carpetas;
   - indexar con hash, metadatos y miniaturas;
   - galería;
   - etiquetas SigLIP;
   - edición manual de etiquetas;
   - filtros y búsqueda;
   - exportación verificada con manifiesto.

   OCR, scanlations, duplicados por similitud y rostros quedan fuera de esta etapa.
2. **Pruebas (etapa 4):**
   - fixtures sintéticos para la integridad;
   - piloto de 100–200 imágenes **elegidas contigo**, para medir la calidad por etiqueta, el OCR, las páginas promocionales y el rendimiento.
3. Biblioteca completa, OCR y scanlations.
4. Rostros, agrupación de obras, duplicados avanzados y videos.

## 10. Skills de desarrollo: estado

| Skill | Estado | Notas |
|---|---|---|
| Skill Creator | ✅ Ya instalada (versión oficial de Anthropic sincronizada desde tu cuenta) | Es más nueva que la del marketplace oficial. No se instaló un duplicado. |
| media-library-catalog | ✅ Creada y evaluada | 100 % frente a 42,5 % sin la skill (4 casos). `AGENTS.md` la hace visible para Gemini. |
| UI/UX Pro Max, Ponytail, Superpowers | ✅ Ya instaladas (nivel usuario) | Sin duplicados. |
| pstack, mattpocock-skills, android, chrisbanes, java-dev | ⏸ Desactivadas **solo en este proyecto** (`.claude/settings.json`) | Duplicaban planificación, TDD y "unslop" de prosa, e inyectaban instrucciones en conflicto. Se reactivan quitando esa línea. |
| GSD | ❌ No se instala | Analizado `open-gsd/gsd-pi`: es un agente autónomo completo (no un plugin), con `postinstall` que descarga Chromium y un binario, y se solapa con Superpowers. La planificación se hace con Superpowers. Opcional: revisar `open-gsd/gsd-core`. |
| Unslop | ⏸ En espera | Aprobada, pero su herramienta audita interfaces web. Se reevalúa cuando exista la interfaz egui. |
| Claude-Mem | ❌ No se instala | Verificado en su código: captura imágenes por defecto y usa Claude como respaldo. Según fuentes externas (sin verificar), no ahorra tokens. Memoria entre sesiones: `AGENTS.md` + `docs/decisiones.md`; `graphify` más adelante solo sobre `src/`. |
