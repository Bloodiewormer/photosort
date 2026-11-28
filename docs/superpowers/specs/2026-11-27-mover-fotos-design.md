# Copiar o mover fotos, recargar la galería y deshacer

Fecha: 2026-11-27. Estado: aprobado por el usuario (opción A).

## Intención

El usuario quiere **separar y ordenar** su biblioteca desde la app. Ejemplo: tomar todas las fotos de una
persona, llevarlas a una carpeta para subirlas a la nube y que **ya no queden** entre las 35.000. Hasta hoy la
app solo creaba copias («Exportar»). Ahora también podrá **mover**.

Esto cambia la regla base del proyecto («los originales son de solo lectura»). La nueva regla es:

> Los originales **nunca se borran**, ni se vuelven a codificar, ni se sobrescriben, ni se les cambian
> metadatos ni fechas. **Solo se mueven** cuando el usuario lo pide expresamente, de forma verificada y
> reversible.

## Objetivos

1. «Exportar» pasa a llamarse **Copiar**. La ventana ofrece **Copiar** o **Mover (cortar)**.
2. El usuario elige la carpeta de salida.
3. Mover no rebaja la calidad ni pierde metadatos: el archivo no se decodifica. Conserva EXIF, IPTC, XMP, ICC,
   GPS, la orientación y las fechas de creación, modificación y acceso.
4. Un botón **Recargar** en la galería vuelve a revisar las carpetas y quita lo que ya no está.
5. **Deshacer** el último movimiento (y los anteriores, de uno en uno).

## No objetivos

- Borrar fotos. Sigue prohibido. No hay botón de borrar ni de vaciar la Papelera.
- Deshacer una copia (eso sería borrarla).
- Subir a la nube. El usuario sube la carpeta de salida con su propia herramienta.

## Anti-objetivos (sería un fracaso aunque funcione)

- Que un original desaparezca sin que exista una copia verificada o sin poder recuperarlo desde la Papelera.
- Que un movimiento sobrescriba un archivo que ya estaba en el destino.
- Que mover cambie un solo byte, una fecha o un atributo del archivo.

## Enfoque elegido: A (copia verificada + Papelera entre discos)

| Opción | Qué hace con el original al mover a otro disco | Elegida |
|---|---|---|
| **A** | Copia verificada con SHA-256, vuelve a comprobar el original y lo manda a la Papelera de Windows | **Sí** |
| B | Copia verificada y deja el original oculto en la galería | No: el original sigue en D: |
| C | Copia verificada y retirada definitiva | No: sin vuelta atrás |

### Mismo disco

Es un renombrado del sistema de archivos. Es atómico y no toca los bytes: conserva todas las fechas, los
atributos y los flujos alternativos.

- Se usa `MoveFileExW` **sin** `MOVEFILE_REPLACE_EXISTING` y **sin** `MOVEFILE_COPY_ALLOWED`.
  `std::fs::rename` **no sirve**: en Windows reemplaza el destino si existe.
- Si Windows responde `ERROR_NOT_SAME_DEVICE`, se aplica el camino entre discos.

### Entre discos

Si algo falla, el original se queda donde estaba y solo se limpia el temporal del destino.

1. Copia verificada, como la exportación actual:
   - se escribe en un `.part` en el destino y se compara su SHA-256 con el del original;
   - se le ponen las fechas del original y se renombra dentro del destino sin sobrescribir.
2. Se vuelve a comprobar el original: mismo tamaño, misma fecha de modificación y mismo SHA-256 que al empezar.
   Si cambió, no se toca y se informa «copiado, original conservado».
3. El original va a la Papelera con `SHFileOperationW` (`FO_DELETE`), como `Supr` en el Explorador:
   - antes se comprueba con `GetVolumePathNameW` y `GetDriveTypeW` que el volumen es un disco fijo
     (`DRIVE_FIXED`). En memorias USB y otras unidades extraíbles, en red o en un CD **no se toca el original**;
   - las banderas son `FOF_ALLOWUNDO | FOF_SILENT | FOF_WANTNUKEWARNING`. Van sin `FOF_NOCONFIRMATION` ni
     `FOF_NOERRORUI`: con la configuración por defecto Windows recicla sin preguntar, y **pregunta siempre**
     antes de borrar algo para siempre (`FOF_NOCONFIRMATION` respondería que sí por el usuario);
   - si el usuario cancela o la llamada falla, el resultado es «copiado, original conservado»;
   - la Papelera **no es duradera**: Windows la vacía cuando se llena o con el Sensor de almacenamiento. La
     protección real es la copia verificada del destino.

## Destino

- **Copiar:** las reglas de hoy. El destino debe estar fuera de las bibliotecas.
- **Mover:** el destino puede estar dentro o fuera de una biblioteca. No puede estar dentro de los datos de la
  app, ni seguir un enlace o junction (como hoy). **Decisión mía:** dentro de una biblioteca, para poder
  ordenar carpetas en D:.
- Sin cambios respecto a la exportación: los diseños de carpetas (plano, por categoría, por año…), los
  conflictos y el espacio libre.
  - Nunca se sobrescribe. Si el nombre está ocupado por otros bytes, se usa `nombre (1).jpg`.
  - Si en el destino ya hay un archivo idéntico (mismo SHA-256), al mover **no se toca el original** y se
    informa «ya estaba en el destino, original conservado». Mi decisión: es lo más seguro.
- Manifiesto CSV/JSON en los dos modos, con una columna nueva `operacion` (`copiar` o `mover`) y la ruta de
  origen (ya existe).
- XMP: el archivo `.xmp` al lado del destino se permite también al mover. **Incrustar etiquetas en el JPEG se
  desactiva al mover**, porque cambiaría los bytes del único ejemplar. Decisión mía.

## Catálogo

- Destino dentro de una biblioteca: la fila de `location` cambia de ruta y de raíz. Las etiquetas siguen
  ligadas al `asset` (al SHA-256), así que no se pierde nada.
- Destino fuera: se borra la fila de `location`. El `asset` y sus etiquetas se quedan. Si algún día se añade
  esa carpeta como biblioteca, reaparecen con todo.
- Si una foto tiene duplicados exactos en otras rutas, solo se mueve la ruta que se ve en la galería. Las
  demás se quedan.

## Registro de movimientos y deshacer

Tabla nueva `move_log` en el catálogo:

```
id, batch, src_path, src_root, dst_path, dst_root, sha256, size, mtime_ns,
method ('rename' | 'copy_recycle'), state ('pending' | 'done' | 'kept_original' | 'undone'), at_ns
```

- **Antes** de tocar un archivo se escribe la fila en `pending`. Después se cambia a `done` (o
  `kept_original`) en la misma transacción que actualiza `location`.
- **Al abrir el catálogo**, cada fila `pending` se resuelve mirando el disco, y solo cuando se ven la carpeta de
  la biblioteca de origen y la carpeta del destino (una unidad desconectada hace que todo parezca borrado):
  - destino presente con el mismo SHA-256 y origen ausente → `done`, con la biblioteca del destino (`dst_root`)
    guardada en la fila;
  - origen presente y destino ausente → se descarta;
  - ambos presentes (se cortó entre la copia y la Papelera) → `kept_original`. No se toca nada;
  - en cualquier otro caso la fila sigue `pending`.
- Si un movimiento falla antes de que cambie nada en el disco, su fila se descarta.
- **Deshacer** revierte el lote más reciente que tenga filas `done`, en orden inverso. Por cada archivo:
  - el archivo del destino tiene que tener el mismo SHA-256 y la ruta original tiene que estar libre. Si no,
    se salta y se informa;
  - si la carpeta original ya no existe, se vuelve a crear;
  - `rename` vuelve con `MoveFileExW`; `copy_recycle` vuelve con el mismo procedimiento entre discos: la copia
    del destino va a la Papelera y el original sigue en la Papelera por si acaso;
  - la fila pasa a `undone` y `location` vuelve a la ruta original.
- Se puede deshacer varias veces seguidas: cada vez, el lote anterior.

## Recargar

Botón **Recargar** en la barra de la galería. Llama a `ReviewEngine::request_index()`, que ya existe para la
revisión periódica de carpetas. Al terminar se recarga la galería, y las fotos marcadas `missing` desaparecen
(la galería ya las filtra). Mientras trabaja, el botón aparece desactivado con el texto «Recargando…».

## Interfaz

- Galería y menú contextual: **Copiar…** en lugar de **Exportar…**.
- La ventana **Copiar o mover**:
  - opción `Copiar | Mover (cortar)`, carpeta de salida, diseño de carpetas y opciones XMP;
  - vista previa con el número de archivos, los bytes, cuántos van **en el mismo disco** (instantáneo) y
    cuántos **a otro disco** (copia + Papelera), los conflictos y el espacio libre (solo cuenta lo que va a
    otro disco).
- **Mover pide una segunda confirmación**: «Se moverán N fotos fuera de sus carpetas. M irán a la Papelera
  después de copiarse y verificarse; si Windows fuera a borrar alguna para siempre, te preguntará antes. Las de
  memorias USB o de red conservan el original. Puedes deshacerlo.» Sin fotos a otro disco, se omite la parte de
  la Papelera.
- El resumen final ofrece **Deshacer este movimiento** y **Abrir carpeta**. La barra de la galería tiene
  **Deshacer último movimiento**, activo cuando hay algo que deshacer.
- Teclado y Narrador: las mismas reglas que la ventana actual (foco inicial en la carpeta, `Esc` cierra si no
  está trabajando).

## Seguridad en el código

- `fsguard` gana un tipo `MoveSource`: una ruta dentro de una biblioteca, archivo normal, no enlace, que el
  catálogo conoce. Solo lo construye el flujo de mover.
- Los movimientos solo existen como funciones de `fsguard`: `move_same_volume(&MoveSource, &DestPath)` y
  `recycle(&MoveSource)` (o la ruta del destino al deshacer).
  - Ningún otro código puede mover ni mandar a la Papelera.
  - `clippy.toml` sigue prohibiendo `std::fs::rename`, `remove_file` y compañía fuera de `fsguard` y `export`.
- Un `DestPath` para mover se crea con una función nueva, `ensure_move_dir`. Permite rutas dentro de una
  biblioteca y rechaza la carpeta de datos de la app y los enlaces.
- Tests ampliados en `crates/app/tests/safety.rs` y en el crate `export`, todos con fixtures sintéticos en
  carpetas temporales:
  - mover en el mismo disco conserva el SHA-256, las fechas (incluida la de creación) y los atributos;
  - nunca se sobrescribe un destino existente;
  - el camino entre discos se fuerza en los tests. La Papelera se sustituye por una función inyectada que mueve
    a una carpeta temporal, para no llenar la Papelera real del usuario;
  - si el original cambia durante la copia, no se toca;
  - deshacer devuelve el archivo idéntico y se niega si la ruta original está ocupada;
  - las filas `pending` se resuelven al abrir.

## Documentos que cambian

- `AGENTS.md`, `.claude/skills/media-library-catalog/SKILL.md` (§1, §4 y §5) y `references/integrity.md` (nuevo
  procedimiento de mover).
- `docs/propuesta-tecnica.md`, `docs/uso.md` y `docs/diseno.md` (textos y atajos).
- `clippy.toml` (comentario de cabecera).
- `docs/decisiones.md`: entrada nueva con fecha, decisión, alternativas y porqué.

## Qué queda para quien lo construya

- Cómo se reparte el código entre `export` y `fsguard`, siempre que se respeten los límites de arriba.
- El texto exacto de los mensajes.

## Durante el desarrollo

Nadie toca las carpetas de D:. Todo se prueba con fixtures sintéticos. El usuario lo prueba él mismo en la app.
