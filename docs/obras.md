# Obras, alias y páginas promocionales

`photosort` representa cada obra o grupo de scanlation como una etiqueta de tipo `entity` (`label.kind = 'entity'`). De este modo, las obras reutilizan directamente los chips de filtro de la galería, la cola de revisión y la exportación por entidad.

## Modelo y garantías

- **Solo lectura sobre originales:** el emparejamiento trabaja únicamente sobre el texto OCR guardado en la base de datos del catálogo y nunca modifica los archivos de la biblioteca (verificado en `crates/app/tests/safety.rs`).
- **Menciones, no pertenencia:** una sugerencia `source='ocr'` significa que la imagen **menciona** la obra en su texto leído. El nombre del archivo **nunca** asigna una obra a una imagen; solo sirve para proponer títulos nuevos que el usuario decide crear o no.
- **Idempotencia y respeto a decisiones:** al re-ejecutar el emparejamiento sobre una imagen, solo se reemplazan las filas previas con `source='ocr'` y `status='suggested'` (entidades y la categoría `Página promocional`). Las asignaciones aceptadas, rechazadas o procedentes de otra fuente (`user`, `ai`, `rules`) no se tocan.
- **Alias:** la tabla `work_alias(label_id, alias, key)` guarda alias adicionales por obra; el nombre principal de la obra actúa siempre como alias implícito.

## Reglas de coincidencia en texto OCR

- **Clave de coincidencia (`work_match_key`):** aplica `ocr_search_key` (NFKD sin diacríticos combinantes `U+0300..=U+036F`, NFC, minúsculas, sin espacios) y conserva únicamente caracteres alfanuméricos, manteniendo intactos los kana con dakuten/handakuten, CJK y Hangul.
- **Candidatos por geometría:** cada línea válida, más cadenas de 2 y 3 líneas apiladas verticalmente según sus cajas (solapamiento horizontal y separación vertical menor a ~1 altura de línea), sin depender del orden de líneas devuelto por el detector y sin cruzar columnas. En imágenes con categoría no rechazada `Capturas de pantalla`, se excluyen las líneas de la barra de estado (`top_strip`).
- **Reglas por longitud de clave (en caracteres):**
  - `< 2`: se ignora.
  - `2..=5`: coincidencia exacta contra un candidato completo (sin subcadena).
  - `>= 6`: coincidencia exacta por subcadena.
  - `>= 10`: además admite coincidencia aproximada con hasta `floor(len / 10)` ediciones, filtrada previamente por principio del palomar (`k + 1` piezas disjuntas) y exigiendo coincidencia exacta de todos los dígitos (p. ej., `"Season 2"` no coincide con `"Season 1"`).
- **Subsunción y confianza:** si la clave de una obra queda contenida en la clave más larga de otra obra sobre las mismas líneas, se conserva solo la más larga. La confianza es la puntuación mínima del OCR de las líneas usadas en coincidencia exacta, o `score * (1 - errores / len)` en aproximada.
- **Página promocional:** sugiere la categoría `Página promocional` (`source='ocr'`) con confianza `0,8` si se mencionan 3 o más obras distintas, `0,7` si aparece una cabecera típica (`proyectos activos`, `proyectos terminados`, `nuestros proyectos`, `otros proyectos`, `current projects`, `ongoing projects`, `completed projects`), o `0,9` si concurren ambas señales.

## Flujo en la interfaz

- **Pestaña «Obras»:** lista virtualizada con filtro, conteo de imágenes y alias, creación manual de obras, gestión de alias, acceso directo a las imágenes de cada obra en la galería (mediante chips), propuesta opcional desde nombres de archivo con marcador de capítulo (`Capítulo`, `Chapter`, `Cap.`, `Ch.`, `Episodio`, `Episode`, `Ep.` seguido de número; nada marcado por defecto) y ejecución en segundo plano de «Buscar obras en el texto leído» con progreso y cancelación.
- **Inspector (sección OCR):** permite marcar líneas leídas y usarlas en un campo editable para «Crear obra con este texto» o «Añadir como alias de…».
- **Integración y concurrencia:** el trabajador de OCR ejecuta el emparejamiento tras guardar cada imagen. El indexado, el OCR y la búsqueda de obras se bloquean mutuamente tanto en la interfaz como en las funciones de arranque.
- **Eliminar obra:** pide confirmación y borra la obra, sus alias y todas sus asignaciones, también las aceptadas o rechazadas. Solo cambia el catálogo. No está disponible mientras haya una tarea en curso, y nunca se hace de forma automática.

## Medido (solo datos sintéticos, 2026-11-26)

- **Páginas promocionales reservadas, con el motor real y DirectML:** 6 páginas con 78 apariciones de título. Como candidatas había 152 obras: las 76 de esas páginas y 76 señuelos del conjunto de ajuste. **Encontradas 77 de 78** y **1 coincidencia errónea**; las 6 páginas se marcaron como promocionales.
- **Velocidad del emparejamiento (`--release`):** 500 obras sobre 45.000 imágenes de 13 líneas, unos 2 s. Incluye comparar el texto, no leer ni escribir la base de datos.

**Límites:** las obras creadas en la prueba llevan el título exacto de la página. Las páginas reales usan tipografías, fondos y disposiciones más difíciles, y los nombres de la lista pueden no coincidir con lo escrito. La app no descubre sola una obra que no esté en la lista.
