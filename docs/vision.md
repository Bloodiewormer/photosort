# Reconocimiento de imágenes y etiquetas visuales

`photosort` analiza las imágenes localmente con **SigLIP 2** (`base-patch16-256`, embeddings de imagen y texto de 768 dimensiones), **YuNet** (detección de rostros con 5 puntos de referencia) y opcionalmente **SFace** (reconocimiento y agrupación de rostros) y guarda los resultados en el catálogo SQLite. A partir de esos vectores guardados, el usuario puede crear, editar, probar y reaplicar reglas de etiquetas **sin volver a leer ninguna imagen del disco**.

## Garantías y privacidad

- **Solo lectura sobre los originales:** las imágenes se abren únicamente para decodificarlas en memoria (`thumbs::decode`, máximo 640 px conservando orientación EXIF). Ningún archivo de la biblioteca se modifica ni se crean archivos junto a ellos (verificado en `crates/app/tests/safety.rs`).
- **100 % local:** ningún modelo se descarga automáticamente y ningún dato sale del equipo.
- **Proveedor compartido y sin caída silenciosa:** OCR y reconocimiento de imágenes comparten una única instancia de ONNX Runtime por sesión (`GPU DirectML` o `CPU`). No hay cambio automático a CPU si falla la GPU; cambiar de dispositivo tras usar uno de los dos motores requiere reiniciar la aplicación.
- **Idempotencia y respeto al usuario:** al aplicar reglas solo se reemplazan las filas `source='ai'` y `status='suggested'` pertenecientes a reglas de visión. Las decisiones del usuario (`accepted`, `rejected`) y las sugerencias de otras fuentes (`rules`, `ocr`, `user`) no se tocan nunca.

## Reglas integradas y puntuación

Al crear un catálogo nuevo se generan una sola vez 16 reglas iniciales, controladas individualmente por `builtin_key` (los catálogos anteriores reciben las reglas nuevas una sola vez sin recrear ni sobrescribir las editadas o eliminadas, y reutilizan cualquier etiqueta existente con el mismo nombre sin sustituir su regla):

- **Tipo de imagen (exclusivo, categoría):** `Manga`, `Ilustración`, `Foto`, `Capturas de pantalla` (nombre exacto compartido con reglas locales y OCR), `Meme` y `Documento`.
- **Categorías independientes de contenido:** `Anime`, `Manhwa`, `Webtoon`, `Animales`, `Computadoras y hardware`, `Tecnología y electrónica`, `Naturaleza y paisajes` y `Videojuegos`. Se combinan libremente con el tipo de imagen (por ejemplo, una imagen puede ser `Capturas de pantalla` y `Videojuegos`).
- **Tipo de página visual (independiente, categoría):** `Portada de manga`, restringida para sugerirse únicamente cuando el tipo ganador es `Manga` o `Ilustración`.
- **Etiqueta independiente:** `persona` (con descripciones negativas para evitar personajes dibujados).

Las descripciones iniciales están en inglés; el usuario puede editarlas o escribir nuevas en español o inglés. Los umbrales y descripciones de tipo y de las categorías de contenido están calibrados con 335 y luego 1000 imágenes reales; `Manhwa` y `Webtoon` vienen desactivadas porque no hubo casos suficientes para medirlas (ver [vision-evaluacion.md](vision-evaluacion.md#calibración-con-imágenes-reales)). `persona`, `retrato` y `grupo de personas` siguen con su umbral inicial.

### Similitud coseno, no probabilidades

Los archivos ONNX divididos de SigLIP 2 no incluyen `logit_scale` ni `logit_bias`. Por ello, **ninguna puntuación ni confianza mostrada es una probabilidad**:

- **Puntuación de una regla:** similitud coseno máxima entre el embedding de la imagen y las descripciones positivas de la regla.
- **Grupo Tipo (exclusivo):** solo se sugiere el tipo activado con mayor similitud coseno si alcanza su umbral. Su confianza de ordenación es `softmax(100 * cosenos)[ganador]`, para que las imágenes más ambiguas aparezcan primero en **Revisión**.
- **Reglas independientes:** se sugieren si la similitud positiva alcanza el umbral y (si hay descripciones negativas) supera la mejor similitud negativa. Su confianza usa una heurística monótona creciente en `[0,50, 0,99]` respecto al margen sobre el umbral.
- **Regla `persona` + YuNet:** `persona` también se sugiere cuando YuNet detecta un rostro con puntuación `≥ 0,80` **y** el tipo ganador es `Foto`, `Meme` o `Capturas de pantalla`. Un rostro dibujado en `Manga` o `Ilustración` no activa `persona`.
- **Reglas geométricas `retrato` y `grupo de personas`:** se evalúan a partir de la geometría de los rostros detectados cuando el tipo ganador es `Foto`:
  - `retrato`: se sugiere si hay exactamente 1 rostro con tamaño al menos de 80×80 píxeles.
  - `grupo de personas`: se sugiere si hay 3 o más rostros con puntuación `≥ 0,80`.
- **Reconocimiento y agrupación de rostros (SFace):**
  - Modelo opcional en `sface/face_recognition_sface_2021dec.onnx` (OpenCV SFace, Apache-2.0). Si no está presente, la detección de rostros sigue funcionando normalmente.
  - Conserva los 5 puntos de referencia faciales dados por YuNet y aplica una transformación de similitud hacia la plantilla estándar de ArcFace de 112×112 en RGB (0..255 sin normalizar, equivalente a `blobFromImage` con `swapRB = true` en OpenCV).
  - Genera un vector normalizado L2 de 128 dimensiones. La app agrupa con un umbral de similitud coseno de **`0,5`**, más estricto que el `0,363` que OpenCV propone para comparar dos caras.
  - **Solo se agrupan caras claras de fotos:** puntuación de YuNet `≥ 0,9`, lado menor de al menos 40 px en la imagen decodificada a 640 px, y nunca en imágenes con tipo `Manga` o `Ilustración` que el usuario no haya rechazado. El inspector muestra solo esas caras.
  - **Medido el 2026-11-27 con la muestra real de 335 imágenes:** YuNet detectó 804 caras; 421 venían de manga e ilustraciones, casi todas con puntuación por debajo de 0,8, y muchas caras de fotos medían menos de 32 px. Con el filtro quedan 151 caras, ninguna de imágenes dibujadas. Con `0,363` el grupo más grande juntaba a dos personas distintas (72 caras); con `0,45` a `0,55` se separan en grupos de 52 y 18 que, revisados a mano, son de una sola persona cada uno. La herramienta `cargo run --release -p app --example calibrar_rostros -- <muestra> <salida> <modelos>` repite la medición y escribe hojas de contacto con las caras numeradas.
  - **Agrupación incremental:** cada rostro se asigna a la persona más parecida si supera el umbral; si no, crea un grupo nuevo sin nombre. La asignación manual de un rostro por el usuario (`user_assigned = 1`) queda fijada y nunca es sobrescrita por análisis posteriores.
  - **Propuesta de nombres:** cuando un grupo sin nombre aparece en 3 o más fotos, la aplicación muestra una tarjeta no intrusiva («Esta persona aparece en N fotos. ¿Cómo se llama?») con muestras de la cara. El usuario puede nombrarla, unirla si el nombre ya existe, posponer la pregunta («Más tarde») o marcarla como ignorada («No preguntar»).
  - **Configuración y privacidad:** la etapa «Agrupar rostros» está desactivada por defecto. Al activarla se calcula el embedding de los rostros ya detectados decodificando únicamente en memoria las imágenes que contienen rostros. La agrupación corre en segundo plano, con progreso y botón «Cancelar» en Configuración; mientras dura, la revisión en segundo plano cede y las demás tareas pesadas esperan. Una foto que no se pueda decodificar se salta. El botón «Borrar datos de rostros» permite reiniciar todos los grupos, nombres y embeddings previa confirmación. Ningún recorte ni dato se escribe junto a los archivos originales ni sale del equipo.
- **Umbrales:** los calibrados se eligieron con una muestra real de 335 imágenes; los demás son provisionales. Usa el botón **Probar** en la pestaña **Etiquetas** para ver las imágenes ordenadas por similitud y elegir el corte adecuado.

## Flujo en la interfaz

- **Menú «Procesar ▾» → «Reconocer imágenes…»:** permite elegir selección actual o carpetas de la biblioteca (ninguna marcada por defecto), seleccionar la carpeta de modelos de visión (`siglip2/` y `yunet/`), elegir el dispositivo compartido (DirectML lote 8 o CPU lote 1), ver progreso «N de M», tiempo restante estimado, cancelar y reanudar. Un error en una imagen no detiene el lote ni a sus compañeras.
- **Pestaña «Etiquetas»:** lista de reglas (tipo exclusivo e independientes), activar/desactivar, editar descripciones y umbral, crear etiquetas personalizadas, eliminar reglas con confirmación (conservando la etiqueta y tus decisiones), **Probar** (ordena la galería por similitud marcando qué imágenes superan el umbral), **Buscar por descripción** en texto libre, **Aplicar a las imágenes analizadas** en segundo plano, y contadores por regla de sugerencias de IA aceptadas y rechazadas.
- **Inspector:** muestra el número de rostros detectados y deja claro que las etiquetas de IA son sugerencias (disponibles también en **Revisión**).
- **Exclusión mutua:** indexado, análisis de reglas, OCR, búsqueda de obras, reconocimiento de imágenes y aplicación de etiquetas no se ejecutan a la vez.

## Alcance de lo verificado (2026-11-26, solo datos sintéticos)

- **Prueba de extremo a extremo con motor real (`vision_e2e`, DirectML lote 8, 4 maquetas sintéticas, 16 reglas integradas con umbrales sin calibrar):** `scene_comic` → `Manga (1,00)`, `Manhwa (0,63)`, `Webtoon (0,77)`; `scene_document` → `Documento (0,99)`; `scene_phone_chat` → `Capturas de pantalla (0,99)`, `Webtoon (0,70)`; `scene_landscape` → `Capturas de pantalla (0,38)`; 0 rostros detectados, `Portada de manga` no se disparó en ninguna maqueta y originales intactos. Nótese que, con el umbral provisional de `0,10` sin calibrar, `Webtoon` también se sugirió en la captura vertical de chat y en el cómic sintético, y `Naturaleza y paisajes` no se disparó en el degradado sintético de paisaje.
- **Rendimiento en `--release` sobre 45.000 activos sintéticos (`benchmark_vision_rank_and_apply_45k_assets`):**
  - Ordenación top-200 por similitud coseno: **media 210,84 ms · p95 231,02 ms**.
  - Aplicación de las 7 reglas a los 45.000 activos: **1,04 s** (39.375 activos con sugerencias, 41.785 sugerencias).
- **El paisaje sintético se clasificó mal:** es un degradado dibujado y se sugirió como captura, con una confianza de ordenación baja. Los otros tres «aciertos» son maquetas muy esquemáticas.
- **Repetición independiente:** mismas cuatro sugerencias; ordenación 192,67 ms de media (p95 200,18 ms) y aplicación en 0,96 s.
- **`DirectML.dll` junto a la app** (1.15.4 incluida): ejecutada la verificación del motor desde `target/release`, dio la misma recuperación, la misma concordancia con CPU y 96 ms por imagen con lote 8.

La vista de **Probar** ordena solo por las descripciones positivas. Superar el umbral no basta para que se sugiera: un tipo además debe ganar a los demás y una etiqueta con descripciones negativas debe parecerse más a las positivas. Crear una etiqueta con el nombre de una regla existente se rechaza, para no reemplazarla.

Las pruebas automáticas y los tiempos medidos usan exclusivamente imágenes sintéticas generadas en código. **No se afirma ninguna precisión sobre fotos reales, manga real ni rostros reales** hasta calibrar y evaluar con una muestra real aprobada por el usuario.
