# Obras, alias y páginas promocionales

`photosort` representa cada obra o grupo de scanlation como una etiqueta de tipo `entity` (`label.kind = 'entity'`). De este modo, las obras reutilizan directamente los chips de filtro de la galería, la cola de revisión y la exportación por entidad.

## Modelo y garantías

- **Solo lectura sobre originales:** el emparejamiento trabaja únicamente sobre el texto OCR guardado en la base de datos del catálogo y los nombres de archivo ya indexados; nunca modifica los archivos de la biblioteca (verificado en `crates/app/tests/safety.rs`).
- **Menciones y pertenencia son relaciones distintas:**
  - **Menciones (`asset_label`, `source='ocr'`):** la imagen menciona la obra en su texto leído (por ejemplo, una página promocional puede mencionar varias obras).
  - **Pertenencia (`asset_work`, «Pertenece a»):** a qué obra, capítulo (texto, conserva formatos como `23.00`) y número de página (opcional) pertenece la imagen, como máximo una obra vigente por activo; cada «No es esta» se recuerda por obra, así que esa obra no vuelve a sugerirse y otra distinta sí puede. Si el activo tiene varias copias, vale el nombre de cualquiera de ellas. Se sugiere únicamente a partir del **nombre de archivo** cuando el título coincide con una obra o alias ya existente (nunca crea obras automáticamente ni se sugiere desde texto OCR).
  - Una página promocional puede pertenecer a la obra A mientras menciona las obras A, B y C.
- **Idempotencia y respeto a decisiones:** al re-ejecutar el emparejamiento sobre una imagen, solo se reemplazan las filas previas `status='suggested'` de la misma fuente (`source='ocr'` para menciones y tipos de página OCR; `source='rules'` para pertenencia por nombre). Las asignaciones aceptadas o rechazadas no se tocan.
- **Alias:** la tabla `work_alias(label_id, alias, key)` guarda alias adicionales por obra; el nombre principal de la obra actúa siempre como alias implícito tanto en menciones OCR como en pertenencia por nombre.

## Reglas de coincidencia en texto OCR

- **Clave de coincidencia (`work_match_key`):** aplica `ocr_search_key` (NFKD sin diacríticos combinantes `U+0300..=U+036F`, NFC, minúsculas, sin espacios) y conserva únicamente caracteres alfanuméricos, manteniendo intactos los kana con dakuten/handakuten, CJK y Hangul.
- **Candidatos por geometría:** cada línea válida, más cadenas de 2 y 3 líneas apiladas verticalmente según sus cajas (solapamiento horizontal y separación vertical menor a ~1 altura de línea), sin depender del orden de líneas devuelto por el detector y sin cruzar columnas. En imágenes con categoría no rechazada `Capturas de pantalla`, se excluyen las líneas de la barra de estado (`top_strip`).
- **Reglas por longitud de clave (en caracteres):**
  - `< 2`: se ignora.
  - `2..=5`: coincidencia exacta contra un candidato completo (sin subcadena).
  - `>= 6`: coincidencia exacta por subcadena.
  - `>= 10`: además admite coincidencia aproximada con hasta `floor(len / 10)` ediciones, filtrada previamente por principio del palomar (`k + 1` piezas disjuntas) y exigiendo coincidencia exacta de todos los dígitos (p. ej., `"Season 2"` no coincide con `"Season 1"`).
- **Subsunción y confianza:** si la clave de una obra queda contenida en la clave más larga de otra obra sobre las mismas líneas, se conserva solo la más larga. La confianza es la puntuación mínima del OCR de las líneas usadas en coincidencia exacta, o `score * (1 - errores / len)` en aproximada.
- **Tipos de página de scanlation:**
  - `Reclutamiento` (`source='ocr'`): frases de búsqueda de staff en español e inglés (`reclutamiento`, `buscamos staff/traductores/cleaners…`, `we are hiring`, `join our team` con contexto de scanlation/roles/Discord), de modo que diálogos como «¡Únete a nosotros!» no se marquen.
  - `Anuncio` (`source='ocr'`): páginas que piden apoyo o dirigen a otro sitio sin ser de reclutamiento (Patreon, Ko-fi, Discord, «visita nuestra página», «síguenos», «apóyanos», «descarga la app» y equivalentes en inglés). Frases curadas y evidencia de contexto para no saltar por una mención suelta de Discord en un diálogo. Si una página es de reclutamiento, no se sugiere también como anuncio.
  - `Créditos` (`source='ocr'`): frases y listas de roles de scanlation en español e inglés (`traducción`/`translator`, `limpieza`/`cleaner`, `typesetter`/`typesetting`, `redibujo`/`redrawer`, `edición`, `corrección`/`proofreader`/`quality check`, `raws`, abreviaturas `TL:`/`CL:`/`TS:`), exigiendo al menos 3 grupos de rol o 2 con señal específica/cabecera/dos puntos para no marcar diálogos normales.
  - `Página promocional` / índice de proyectos (`source='ocr'`): confianza `0,8` si se mencionan 3 o más obras distintas, `0,7` si aparece una cabecera típica (`proyectos activos`, `proyectos terminados`, `nuestros proyectos`, `otros proyectos`, `current projects`, `ongoing projects`, `completed projects`), o `0,9` si concurren ambas señales.
  - `Portada de manga` (`source='ai'`): clasificada por visión mediante reglas de CLIP/SigLIP2.
  - `Página de capítulo` (`source='rules'`): se sugiere cuando el nombre del archivo contiene un marcador de capítulo (`parse_work_from_stem`) y la página no es promocional, de créditos, de reclutamiento, de anuncio ni portada. Se procesa en la etapa de reglas y en «Buscar obras» a partir del nombre del archivo ya indexado, sin lecturas adicionales de la biblioteca.
  - **Sugerencia única y prioridad:** a lo sumo una sugerencia de tipo de página por imagen cuando haya varias candidatas concurrentes. Prioridad estricta: `Reclutamiento > Anuncio > Créditos > Página promocional > Portada de manga > Página de capítulo`.
  - **Respeto a decisiones del usuario:** las asignaciones aceptadas o rechazadas por el usuario nunca se sobrescriben por análisis posteriores.
  - **Qué no se detecta:** páginas de créditos o reclutamiento puramente visuales sin texto legible, anuncios comerciales genéricos o avisos en idiomas distintos del español e inglés.

## Flujo en la interfaz

- **Pestaña «Obras»:** lista virtualizada con filtro, conteo separado de imágenes que **pertenecen** a cada obra y **menciones**, gestión de alias, acceso a las páginas de la obra ordenadas por capítulo y página en la galería, confirmación masiva (con paso de confirmación) de sugerencias pendientes de pertenencia, propuesta opcional desde nombres de archivo y ejecución en segundo plano de «Buscar obras en el texto leído» (que actualiza menciones OCR y pertenencia por nombre).
- **Inspector:** muestra «Pertenece a» con obra, capítulo, página y acciones «Confirmar» / «No es esta», además de la sección OCR para crear obras o añadir alias.
- **Exportación:** el manifiesto (CSV y JSON) incluye la relación `belongs_to` (obra, capítulo y página) cuando no está rechazada.
- **Integración y concurrencia:** la etapa `rules` y «Buscar obras» sugieren pertenencia por nombre de archivo; el trabajador de OCR ejecuta el emparejamiento de menciones y tipos de página tras guardar cada imagen.
- **Eliminar obra:** pide confirmación y borra la obra, sus alias, sus filas de pertenencia (`asset_work`) y todas sus asignaciones, también las aceptadas o rechazadas. Solo cambia el catálogo. No está disponible mientras haya una tarea en curso, y nunca se hace de forma automática.

## Medido (solo datos sintéticos, 2026-11-26)

- **Páginas promocionales reservadas, con el motor real y DirectML:** 6 páginas con 78 apariciones de título. Como candidatas había 152 obras: las 76 de esas páginas y 76 señuelos del conjunto de ajuste. **Encontradas 77 de 78** y **1 coincidencia errónea**; las 6 páginas se marcaron como promocionales.
- **Velocidad del emparejamiento (`--release`):** 500 obras sobre 45.000 imágenes de 13 líneas, unos 2 s. Incluye comparar el texto, no leer ni escribir la base de datos.

**Límites:** las obras creadas en la prueba llevan el título exacto de la página. Las páginas reales usan tipografías, fondos y disposiciones más difíciles, y los nombres de la lista pueden no coincidir con lo escrito. La app no descubre sola una obra que no esté en la lista.
