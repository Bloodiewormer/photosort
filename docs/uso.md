# Guía de uso de photosort

photosort cataloga tus fotos y vídeos en tu propio equipo. Nunca borra ni modifica los archivos originales, y solo los mueve si tú lo pides. Todas las etiquetas, categorías, obras y personas viven en el catálogo de la aplicación, y solo se copian o se mueven archivos si tú lo decides.

## Primeros pasos

Al abrir photosort por primera vez aparece la pantalla «Elige las carpetas con tus fotos». Pulsa **Añadir carpeta…** y elige una carpeta de tu biblioteca. Más adelante puedes añadir o quitar carpetas desde **Configuración → Carpetas con fotos**.

Cuando añades una carpeta o pulsas **Buscar cambios ahora** en Configuración, la aplicación indexa su contenido en modo de solo lectura:

- Lee cada foto y cada vídeo para calcular su huella de contenido (SHA-256). Por eso, **los vídeos se leen enteros una vez** durante el primer indexado.
- En pasadas siguientes, si un archivo no ha cambiado de ruta, tamaño ni fecha de modificación, no se vuelve a leer entero.
- **Fecha de último acceso en Windows:** al leer un archivo, Windows puede actualizar su fecha de último acceso según la configuración del sistema. El contenido y las fechas de creación y modificación del original nunca cambian.
- Si quitas una carpeta con **Quitar** (y confirmas en **Quitar de la app**), deja de mostrarse en la aplicación pero no se borra ningún archivo. Si vuelves a añadirla más tarde, recuperas tus etiquetas y decisiones.

## Activar los modelos

photosort no incluye ni descarga modelos de reconocimiento. Para usar el análisis visual o la lectura de texto, descarga primero los archivos indicados en el [README](../README.md#modelos) y guárdalos en una carpeta fuera de tu biblioteca (por ejemplo, con las subcarpetas `siglip2`, `yunet`, `sface` y `ocr`).

En **Configuración → Modelos y dispositivo**:

1. En **Modelos de visión (SigLIP 2 y YuNet)** pulsa **Elegir carpeta…** y selecciona la carpeta de modelos. La aplicación comprueba los archivos (incluso si eliges una carpeta padre o una subcarpeta) y activa la etapa **Reconocer imágenes**.
2. Si existe una subcarpeta `ocr` válida junto a ellos (hija o hermana), **Modelos OCR** se rellena sola y se activa **Leer texto**. Si prefieres elegir primero la carpeta de OCR, también comprueba la carpeta padre para visión.
3. En **Dispositivo** elige **GPU (DirectML)** o **CPU**. Solo se usa un proveedor por sesión: cambiarlo tras haber usado un modelo requiere reiniciar la aplicación. Si falla la GPU, la app avisa y nunca cambia a CPU en silencio.

En **Configuración → Revisión en segundo plano** puedes encender **Revisar fotos en segundo plano** (viene desactivado por defecto) y elegir qué etapas se ejecutan:

| Etapa | Qué hace |
|---|---|
| **Reglas rápidas** | Activada por defecto. Detecta capturas, WhatsApp, GIF, dimensiones, formato y fecha de captura EXIF sin usar modelos. |
| **Reconocer imágenes** | Clasifica tipo de imagen, categorías de contenido, etiquetas visuales y detecta rostros con SigLIP 2 y YuNet. En vídeos analiza varios fotogramas repartidos por su duración. |
| **Leer texto** | Lee texto en español, inglés, japonés, chino y coreano, y busca obras y tipos de página. No se ejecuta en vídeos. Es la etapa más lenta. |
| **Agrupar rostros** | Agrupa rostros claros de fotos por persona con el modelo opcional SFace (`sface/face_recognition_sface_2021dec.onnx` dentro de la carpeta de modelos de visión). |

En la barra superior verás el progreso de la revisión y los botones **Pausar** y **Reanudar**. Una foto pasa a **Revisadas** (y muestra la marca ocre en la esquina superior derecha) cuando completa todas las etapas que tengas activadas.

## Seleccionar fotos

Cada miniatura tiene un círculo en la esquina superior derecha: aparece al pasar el ratón y, mientras haya algo seleccionado, se ve en todas. Un clic en el círculo añade o quita esa foto sin soltar las demás, como `Ctrl`+clic. Una foto seleccionada lleva el círculo ocre con `✓`, un borde grueso y un velo sobre la miniatura; si además está revisada, la marca de lápiz graso rodea el círculo. Un clic en el resto de la miniatura selecciona solo esa foto, y un clic en un espacio vacío de la cuadrícula quita la selección. Con teclado, `Ctrl`+`Espacio` sobre una miniatura enfocada hace lo mismo que el círculo.

## Revisar sugerencias

Todo lo que detecta la aplicación entra como sugerencia. Lo que aceptas o rechazas queda guardado y ningún análisis posterior lo sobrescribe.

- **Vista Sugerencias:** en el panel izquierdo (**Grupos por etiqueta**) elige una etiqueta o categoría. En cada fila puedes pulsar **Aceptar** o **Rechazar**, o usar **Aceptar todo el grupo** y **Rechazar todo el grupo** (piden confirmación antes de aplicarse). Arriba tienes **Umbral de confianza** para filtrar qué sugerencias ver y **Deshacer (Ctrl+Z)** para revertir la última decisión.
- **Inspector de la galería:** al seleccionar una foto en la galería, el panel derecho muestra sus etiquetas en **Etiquetas** con botones **Aceptar** y **Rechazar**.
- **Volver a revisar fotos concretas:** en la galería, haz clic derecho sobre una foto o una selección y elige **Revisar** o **Volver a revisar** (también disponible en la barra de selección, en el inspector y con `Mayús+F10` o la tecla de menú). Funciona incluso con **Revisar fotos en segundo plano** apagado; si cierras la app antes de que termine con ese interruptor apagado, las fotos pendientes quedan en **Sin revisar**.
- **Lista de errores:** si una foto o vídeo falla en una etapa (por ejemplo, un archivo dañado o un formato sin códec instalado en Windows), el fallo se guarda para no repetirlo en bucle. Abre la lista desde el contador de errores de la barra superior o desde **Configuración → Errores de revisión → Ver errores**. Allí puedes pulsar **Reintentar** en una fila concreta o **Reintentar todos**.

## Etiquetas

La vista **Etiquetas** reúne las reglas visuales automáticas y la gestión del catálogo de categorías y etiquetas.

### Probar reglas y ajustar el umbral

1. En la lista central aparecen las reglas agrupadas en **Tipo de imagen (exclusivo)**, **Categorías independientes** y **Etiquetas independientes**. Puedes activar o desactivar cada regla con su casilla.
2. Selecciona una regla para abrir su editor a la derecha. Puedes editar sus **Descripciones positivas (una por línea)** y **Descripciones negativas (opcional, una por línea)** en inglés o español, y ajustar el **Umbral de similitud coseno**.
3. Pulsa **Probar** en la fila o **Probar en la galería** en el editor. La galería se ordena por parecido con esa regla y marca con `✓` las imágenes que superan el umbral. Cuando termines, pulsa **Volver a la galería**.
4. Pulsa **Guardar cambios** y después **Aplicar a las imágenes analizadas** arriba. Las sugerencias se recalculan con los vectores ya guardados, sin volver a leer las fotos del disco.

Para crear una regla visual nueva, pulsa **Nueva etiqueta…**, escribe su nombre y descripciones y confirma con **Crear etiqueta**. Si ya no necesitas una regla, usa **Eliminar regla…** en el editor (se conservan la etiqueta y tus decisiones previas). Desde **Buscar por descripción** y **Buscar similares** puedes ordenar la galería por cualquier frase libre sin crear una regla.

### Categorías, subcategorías y etiquetas del catálogo

En la sección **Categorías y etiquetas del catálogo** (arriba en **Etiquetas**) puedes pulsar **Nueva categoría** o **Nueva etiqueta**. En cada fila, el menú **Acciones** (o clic derecho sobre la fila o sobre un filtro en la barra lateral) ofrece:

- **Renombrar:** cambia el nombre. Si ya existe otro igual, ofrece **Fusionar** o **Cambiar nombre**.
- **Crear subcategoría** y **Mover a…** (solo categorías): organiza categorías en árbol o devuélvelas al **Nivel raíz (sin categoría padre)**. Al filtrar por una categoría se incluyen automáticamente sus subcategorías.
- **Fusionar con…:** reúne dos categorías o dos etiquetas en una sola, conservando tus decisiones (aceptada prevalece sobre rechazada, y esta sobre sugerida).
- **Eliminar…:** quita la categoría o etiqueta solo del catálogo (si tenía subcategorías, suben un nivel). Ningún archivo se modifica.

También puedes añadir una etiqueta, categoría o entidad manualmente a una foto desde el inspector derecho, eligiendo el tipo junto al campo **Nueva etiqueta** y pulsando **Añadir**.

## Personas

Para agrupar caras por persona necesitas haber analizado las fotos con **Reconocer imágenes** y tener el archivo opcional de SFace dentro de la carpeta de modelos de visión.

1. Activa **Agrupar rostros** en **Configuración → Revisión en segundo plano**. La agrupación corre en segundo plano con barra de progreso y botón **Cancelar** (si se interrumpe, puedes relanzarla con **Agrupar ahora**). Solo agrupa caras claras de fotos; las caras dibujadas de manga o ilustraciones se omiten.
2. Abre la vista **Personas** en la barra lateral. Cuando un grupo sin nombre aparece en 3 o más fotos, aparece arriba una tarjeta («Esta persona aparece en N fotos. ¿Cómo se llama?»): escribe el nombre y pulsa **Guardar** (si el nombre ya existe, ofrece **Unir grupos**), o elige **Más tarde** o **No preguntar**.
3. En cada tarjeta puedes pulsar **Ver fotos**, **Renombrar** u **Ocultar** (para ver las ocultas marca **Mostrar ocultas** arriba).
4. **Unir grupos con «Misma persona»:** si la misma persona aparece en dos o más tarjetas, marca la casilla **Misma persona** en cada una y pulsa **Unir** en la barra superior.
5. Desde el inspector de una foto, la sección **Personas en esta foto** permite pulsar **Quitar** para desvincular esa foto de una persona o **Poner nombre** para ir a la vista Personas.
6. **Borrar datos de rostros:** en **Configuración → Revisión en segundo plano**, el botón **Borrar datos de rostros** (confirmando con **Sí, borrar datos de rostros**) elimina todos los grupos, nombres y vectores faciales sin afectar a las etiquetas ni a los archivos originales.

## Obras

La vista **Obras** sirve para organizar obras de manga, manhwa o grupos de scanlation.

- **Crear obras y alias:** escribe un título en **Nueva obra o grupo…** y pulsa **Crear obra**, o pulsa **Proponer desde nombres de archivo…** para detectar títulos en archivos con marcador de capítulo, marcar los que quieras y pulsar **Crear seleccionadas**. También puedes seleccionar una foto en la galería, marcar líneas en **Texto** del inspector y pulsar **Crear obra** o **Añadir alias**. Al seleccionar una obra en **Obras**, añade o quita variantes en **Alias de la obra** (el nombre principal siempre se busca automáticamente).
- **Buscar obras en el texto leído:** pulsa este botón en la barra superior de **Obras**. Busca menciones en el texto OCR guardado, detecta tipos de página (`Reclutamiento`, `Anuncio`, `Créditos`, `Página promocional`), comprueba pertenencia por nombre de archivo, actualiza grupos por carpeta y detecta portadas en páginas promocionales.
- **Pertenencia frente a menciones:** son relaciones separadas. **Pertenece a** indica de qué obra, capítulo y página forma parte un archivo (se sugiere por nombre de archivo o carpeta, nunca por texto OCR). **Menciones** son las obras que aparecen escritas en la imagen (una página promocional puede pertenecer a una obra y mencionar muchas otras). En **Obras** puedes abrir cada vista con **Ver páginas por capítulo** y **Ver menciones**, confirmar pertenencias pendientes con **Confirmar sugerencias**, o confirmar y descartar una a una en el inspector (**Confirmar** / **No es esta**).
- **Grupos propuestos:** cuando una carpeta contiene páginas de una misma obra (por el nombre de la carpeta o por mayoría), aparece en **Grupos propuestos** con miniaturas de muestra y botones **Aceptar grupo** y **Descartar grupo**.
- **Portadas:** en imágenes marcadas como página promocional, el inspector dibuja los rectángulos numerados de las portadas sobre la vista previa y muestra la sección **Portadas** con la obra asociada y los botones **Confirmar** y **No es esta**.

## Buscar y filtrar

- **Búsqueda de texto:** el cuadro **Buscar en texto o ruta (/)** de la barra superior busca tanto en el texto leído por OCR (omitiendo la barra de estado en capturas de pantalla) como en la ruta de los archivos.
- **Filtros de la barra lateral:** debajo de la navegación están los grupos plegables **Tipo de medio** (**Fotos**, **Vídeos**), **Categorías**, **Etiquetas**, **Entidades**, **Formato**, **Orientación** (**Horizontal**, **Vertical**, **Cuadrada**), **Año de captura** y **Personas**.
- **Modos de filtro en categorías, etiquetas y entidades:** un clic alterna entre exigir (Y), incluir alguna (O), excluir (NO) o quitar. Con clic derecho puedes elegir directamente **Exigir**, **Incluir alguna**, **Excluir** o **Quitar filtro**.
- **Orden y búsqueda visual:** en la galería, el desplegable **Orden** ordena por ruta, fecha de captura, tamaño en píxeles, tamaño en disco o modificación. Para buscar imágenes parecidas a una foto analizada, haz clic derecho sobre ella y elige **Buscar parecidas**.

## Duplicados

La vista **Duplicados** es de solo lectura: nunca borra, mueve ni renombra archivos. Cada tarjeta ofrece **Mostrar en el Explorador** para localizar el archivo en Windows.

- **Duplicados exactos:** agrupa archivos con el mismo SHA-256 (contenido idéntico), ordenados por espacio duplicado.
- **Parecidos:** compara los vectores visuales guardados (requiere al menos 2 fotos analizadas con **Reconocer imágenes**). Ajusta **Umbral de similitud** (entre 0,80 y 0,99) y pulsa **Buscar parecidos**. Cada grupo incluye **Ver en galería**.

## Copiar o mover

**Copiar** crea copias nuevas en la carpeta que elijas y deja los originales donde están. **Mover (cortar)** se lleva el único ejemplar a esa carpeta, para separar y ordenar. Ninguna de las dos borra, sobrescribe ni vuelve a codificar un archivo.

1. En la galería, selecciona las fotos (o pulsa `Ctrl+A` para seleccionar todos los resultados visibles) y pulsa **Copiar o mover…** en la barra de selección o en el menú contextual.
2. En la ventana **Copiar o mover**, elige **Copiar** o **Mover (cortar)** y la **Carpeta de destino** con **Elegir carpeta…**. Para copiar debe estar fuera de todas tus bibliotecas registradas. Para mover puede estar dentro de una (para ordenar carpetas) o fuera, pero no en la carpeta de datos de photosort.
3. Elige la **Estructura de carpetas**:
   - **Plana:** todas las copias directamente en la carpeta de destino.
   - **Por categoría:** una subcarpeta por la primera categoría de cada imagen (o «Sin categoría»).
   - **Por obra:** una subcarpeta por la primera obra o entidad (o «Sin obra»).
   - **Por año:** una subcarpeta por el año de modificación del archivo (o «Sin fecha»).
   - **Conservar carpetas:** replica la estructura original de carpetas respecto a su biblioteca (si exportas desde varias bibliotecas, añade el nombre de cada una como primer nivel).
4. Opciones de **Metadatos y etiquetas** (desactivadas por defecto):
   - **Guardar etiquetas en un archivo XMP junto a cada archivo del destino:** crea un archivo `.xmp` al lado de cada copia (o de cada foto movida) con categorías, jerarquía, etiquetas, obras y personas con nombre. Solo se crea en la carpeta de destino.
   - **Incrustar etiquetas en las copias JPEG (cambia sus bytes; el original no se toca):** inserta las etiquetas XMP en la copia JPEG después de verificarla, sin recomprimir la imagen. El manifiesto guarda la huella SHA-256 antes y después. No está disponible al mover.
5. Pulsa **Calcular vista previa** para revisar cuántos archivos se copiarán o moverán, el tamaño total, el espacio libre en destino, los conflictos de nombre (se renombran con sufijo sin sobrescribir) y los archivos ya presentes (se omiten si tienen el mismo SHA-256).
6. Para copiar, pulsa **Copiar**. Cada archivo se copia byte a byte, se verifica con SHA-256, conserva sus fechas y se acompaña de dos manifiestos (CSV y JSON), con la columna `operacion`. Al terminar puedes pulsar **Abrir carpeta**.
7. Para mover, pulsa **Mover…**, lee la segunda confirmación y pulsa **Mover**. Ahí `Intro` no hace nada, salvo que lleves el foco al botón **Mover**.

### Qué pasa al mover

- La vista previa dice cuántos archivos van **en el mismo disco** (instantáneo: no cambian ni los bytes, ni las fechas, ni los atributos) y cuántos **a otro disco**. Estos se copian, se verifican con SHA-256, se comprueba que el original no ha cambiado y solo entonces el original va a la **Papelera de Windows**, como al pulsar `Supr` en el Explorador. Si Windows fuera a borrarlo para siempre (por ejemplo, porque no cabe en la Papelera), te pregunta antes: si respondes que no, la copia se queda y el original también.
- Si el original está en una **memoria USB** u otra unidad extraíble, en una **carpeta de red** o en un disco óptico, no se toca: se copia, se verifica y el original **se queda donde estaba**.
- La Papelera **no es un lugar seguro a largo plazo**: Windows la vacía cuando se llena o con el Sensor de almacenamiento. Lo que protege tu foto es la copia verificada del destino.
- Entre discos se conservan las fechas y los atributos de solo lectura, oculto y sistema, pero no los flujos alternativos ni los permisos de Windows (el original de la Papelera los mantiene).
- Si en el destino ya hay un archivo idéntico, el original **no se toca**. Si el nombre lo ocupa otro archivo distinto, se usa `nombre (1)`.
- Si el resumen dice **Copiado, original conservado**, hay una copia en el destino y el original sigue en su sitio, con el motivo.
- Si un corte de luz deja un archivo `.part` en el destino, photosort no lo borra ni lo reutiliza. El original sigue en su sitio, así que puedes borrar ese `.part` tú mismo.
- Al mover no se incrustan etiquetas en los JPEG. El archivo `.xmp` al lado es opcional.
- Las etiquetas siguen con la foto. Si la mueves fuera de tus bibliotecas, deja de verse en la galería; si añades esa carpeta como biblioteca, reaparece con todo.
- Si una foto tiene duplicados exactos en otras rutas, solo se mueve la que ves en la galería.
- Puedes cancelar a mitad: lo que ya se movió se puede deshacer y el resto no se toca.
- Los accesos directos, las carpetas, los enlaces y los archivos de OneDrive «solo en la nube» (u otros marcadores de posición) no se mueven.

### Deshacer un movimiento

**Deshacer este movimiento**, en el resumen, o **Deshacer último movimiento**, en la barra de la galería, devuelven cada archivo a su ruta original; las veces siguientes, el movimiento anterior. Un archivo solo vuelve si el del destino no ha cambiado y la ruta original está libre. Si el del destino ya no está o cambió, o en la ruta original hay otro archivo, ese ya no se puede deshacer: se te dice dónde quedó, no se toca nada y no impide deshacer los movimientos anteriores. Si el problema es pasajero (una unidad desconectada o un archivo abierto en otro programa), se puede reintentar, y mientras tanto no se pasa al movimiento anterior. Mientras se deshace, **Copiar o mover…** queda desactivado. Si volvía desde otro disco, la copia del destino va a la Papelera. Una copia no se puede «deshacer», porque sería borrarla.

## Recargar

El botón **Recargar** de la galería vuelve a revisar tus carpetas y quita las fotos que ya no están (por ejemplo, las que moviste o borraste fuera de photosort). Sus etiquetas se conservan. Mientras trabaja dice **Recargando…**; si la revisión está en pausa, se hará al reanudarla.

## Teclado y accesibilidad

### Atajos de teclado

| Dónde | Atajo | Acción |
|---|---|---|
| En toda la app | `/` o `Ctrl+F` | Enfoca el cuadro de búsqueda |
| En toda la app | `Tab` / `Mayús+Tab` | Recorre los controles con borde de foco visible |
| Galería | Flechas (`Mayús` + flechas) | Mueve la selección (o la amplía) |
| Galería | `Ctrl+A` | Selecciona todas las fotos visibles |
| Galería | `Ctrl`+`Espacio` sobre una miniatura enfocada | Añade o quita esa foto sin quitar las demás |
| Galería | `Intro` o `Espacio` | Abre o cierra el visor |
| Galería | `Tecla de menú` o `Mayús+F10` | Abre el menú contextual de la foto |
| Galería | `Esc` | Cierra el visor, el menú o quita la selección |
| Galería | Clic en un espacio vacío de la cuadrícula | Quita la selección |
| Sugerencias | `A` / `R` | Acepta o rechaza la sugerencia actual |
| Sugerencias | `↑` / `↓` y `←` / `→` | Cambia de fila o de grupo |
| Sugerencias | `Ctrl+Z` | Deshace la última decisión |
| Copiar o mover | `Esc` | Cierra la ventana (cuando no está copiando ni moviendo) |

### Probar con Narrador de Windows

La interfaz expone sus controles mediante AccessKit y UI Automation. Para comprobarla con Narrador usando los datos de demostración (`photosort --demo`):

1. Activa Narrador (`Ctrl+Windows+Intro`) y recorre la ventana con `Tab`: cada parada debe anunciar su nombre en español y su tipo (botón, interruptor, campo de edición).
2. Pulsa una flecha en la galería para seleccionar la primera foto y llega con `Tab` a una miniatura: debe oírse el nombre del archivo, su estado y si está seleccionada.
3. Con una foto seleccionada, pulsa la tecla de menú o `Mayús+F10`: Narrador debe leer cada orden al moverte con las flechas, y `Esc` cierra el menú.
4. En **Configuración**, comprueba que cada interruptor anuncia qué controla y si está activado, y que Apariencia y Dispositivo indican cuál está elegido.
5. Abre **Copiar o mover…**: el foco debe empezar en **Carpeta de destino**, y `Esc` cierra la ventana.
6. En **Personas** y **Etiquetas**, comprueba que los campos de texto y el umbral se anuncian con su nombre.
7. Repite el recorrido con `Tab` en los temas **Claro** y **Oscuro** (en **Configuración → Apariencia**) para comprobar que el borde de foco se distingue en ambos.

## Privacidad

Todo el análisis se ejecuta en tu equipo. photosort no envía fotos, texto leído, rutas, fechas ni rostros fuera del PC, no usa APIs de pago y no incluye telemetría.

Los datos de la aplicación se guardan en `%LOCALAPPDATA%\photosort` (puedes abrirla desde **Configuración → Acerca de → Mostrar en el Explorador**):

- `catalog.sqlite`: base de datos del catálogo (rutas, huellas SHA-256, etiquetas, categorías, obras, texto OCR, vectores visuales y datos de rostros). Trátala como información privada.
- `settings.json`: carpetas registradas y preferencias.
- `thumbs/`: caché de miniaturas. Se puede borrar y se regenera sola sin perder nada del catálogo.
