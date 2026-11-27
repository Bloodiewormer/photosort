# Revisión en segundo plano

`photosort` revisa las fotos de las carpetas registradas en segundo plano solo cuando el usuario activa la opción en **Configuración → Revisión en segundo plano** (desactivada por defecto), o cuando pide **Revisar / Volver a revisar** sobre una o varias fotos concretas.

## Qué significa «Revisada»

No existe una bandera fija en la base de datos. Una foto cuenta como **revisada** únicamente si tiene completadas en la tabla `analysis`, con su versión actual, **todas las etapas activadas** por el usuario:

1. **Reglas rápidas** (`rules`, activada por defecto): lee cabeceras y metadatos EXIF para sugerir capturas de pantalla, WhatsApp, GIF, formato vertical largo y fotos de cámara. No requiere modelos.
2. **Reconocer imágenes** (`vision`, desactivada por defecto): calcula embeddings con SigLIP 2, detecta rostros con YuNet y aplica las reglas visuales. Requiere carpeta de modelos de visión; al elegir una carpeta válida se activa la etapa y, si existe una carpeta `ocr` hija o hermana válida, se rellena también la de OCR.
3. **Leer texto** (`ocr`, desactivada por defecto): extrae texto con OCR y ejecuta el emparejamiento de obras y páginas promocionales. Es la etapa más lenta y requiere carpeta de modelos OCR; al elegir una carpeta válida se activa la etapa y se comprueba su carpeta padre para visión. Activar cualquiera de las dos etapas sin carpeta abre directamente el selector.

Si el usuario activa una etapa más adelante, las fotos a las que les falte pasan automáticamente a **Sin revisar**. Si no hay ninguna etapa activada, ninguna foto cuenta como revisada.

En la galería, cada foto revisada lleva la marca ocre de lápiz graso en la esquina superior derecha (con descripción «Revisada»).

## Ejecución, memoria y seguridad

- **Solo lectura sobre originales:** el motor solo abre los archivos de la biblioteca para leerlos. Windows puede actualizar la fecha de último acceso al leer un archivo, y Configuración avisa de ello.
- **Lotes progresivos sin compartir VRAM:** al arrancar con la revisión activada, primero busca cambios en las carpetas registradas y después procesa las fotos pendientes en lotes (128 fotos por defecto), completando todas las etapas activadas de un lote antes de pasar al siguiente. Nunca mantiene cargados a la vez los motores de visión y OCR en la GPU (4 GB de VRAM); si solo está activada una de las dos etapas GPU, su motor permanece cargado entre lotes sin recargarse. Con las dos activadas, cada lote vuelve a cargar los motores: en la compilación optimizada, con DirectML y datos sintéticos, se midió una vez 5,3 s para visión y 0,7 s para OCR (en la compilación de depuración la carga de visión tarda unos 25 s).
- **Errores del catálogo:** si la base de datos rechaza una escritura (disco lleno, bloqueo), la foto se aparta y el motor espera 30 s antes de seguir; pausar o reintentar acorta la espera.
- **Fallos persistentes sin bucles:** si una foto falla en una etapa, se guarda en `stage_error` (motivo breve saneado, última fecha y número de intentos; el motivo nunca contiene texto OCR, rutas ni nombres de archivo) y se omite incluso en sesiones posteriores para no repetirla en bucle. Solo se guarda si el archivo se puede abrir: una unidad desconectada, un archivo bloqueado o un error de escritura del catálogo apartan la foto únicamente durante la sesión. Cuando la etapa tiene éxito más adelante o el usuario pide volver a revisar la foto, el registro se borra. Desde el contador de errores de la barra superior y desde **Configuración → Errores de revisión** se abre la lista de fotos con errores (ocultando carpetas quitadas), con **Reintentar** por foto y para todas. Si falla la carga de un modelo o falta su carpeta, esa etapa se pausa con el motivo visible y sin caer a CPU en silencio, mientras las demás etapas activas continúan.
- **Pausa y exclusión mutua:** **Pausar** detiene el motor tras la foto en curso y **Reanudar** continúa. Cuando se inicia una tarea pesada en primer plano (buscar obras o aplicar reglas de etiquetas), el motor cede el paso automáticamente y reanuda al terminar.
- **Volver a revisar:** desde el menú contextual (clic derecho o `Mayús+F10`), la barra de selección o el inspector, reinicia únicamente las filas de `analysis` de las etapas activas y procesa esas fotos primero, incluso con el interruptor general apagado. Si se pausa o se cambia un ajuste, las que faltan vuelven a la cola. La cola vive en memoria: **al cerrar la app, con el interruptor apagado, las pedidas que no se hayan revisado quedan como «Sin revisar» y hay que pedirlas otra vez**. Las decisiones del usuario (`accepted`/`rejected`), el texto y los vectores anteriores se conservan hasta que lleguen los resultados nuevos. Las sugerencias que aún no has decidido sí pueden cambiar.
- **Carpetas quitadas:** el motor solo abre copias que estén dentro de carpetas registradas, aunque la misma foto esté también en una quitada. Quitar una carpeta en Configuración la excluye de galería, contadores, sugerencias, duplicados, búsqueda, exportación y revisión, pero no borra ningún archivo ni elimina sus filas del catálogo; volver a añadirla restaura todo.

## Verificado con los modelos reales

`crates/app/tests/review_e2e.rs` ejecuta el motor como lo hace la app, con DirectML, las tres etapas, 4 imágenes sintéticas y lotes de 2: los motores se turnan (visión, OCR, visión, OCR), las 4 quedan revisadas sin errores, se lee texto y los originales no cambian. No mide una biblioteca grande, la memoria de la GPU ni fotos reales.
