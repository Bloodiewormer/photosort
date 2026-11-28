# Integridad: lectura segura, copia verificada y movimiento reversible

El proyecto está escrito en Rust. Los fragmentos son bocetos de la intención: manda el código real y sus tests.

## Guardián de rutas

`DestPath` es un tipo que solo construyen `ensure_writable`, `ensure_export_dir` y `ensure_move_dir` (ver «Mover»). Toda función que escribe, crea directorios, renombra o borra un temporal recibe un `DestPath`, nunca un `&Path`. Así, escribir sin pasar por el guardián **no compila**.

```rust
pub struct DestPath(PathBuf); // constructor privado: solo lo crea ensure_writable

pub fn ensure_writable(path: &Path, roots: &[LibraryRoot]) -> Result<DestPath, GuardError> {
    // 1. dunce::canonicalize del ancestro existente más cercano (resuelve symlinks y
    //    junctions) y se vuelve a añadir el resto de la ruta.
    // 2. Comparación componente a componente y en minúsculas (NTFS no distingue mayúsculas).
    // 3. Err si la ruta es una raíz de biblioteca o cae dentro de una.
}
```

- Se usa antes de **cada** `create_dir`, apertura en escritura, `rename`, borrado de temporales y fijado de fechas.
- Se comprueba la ruta *resuelta*. Un junction dentro del destino que apunte a la biblioteca también se rechaza.
- Al configurar la exportación, el destino no puede estar dentro de una raíz, y ninguna raíz puede estar dentro del destino.
- `clippy.toml` prohíbe con `disallowed-methods` las APIs destructivas de `std::fs`. Solo `fsguard` y la crate `export` pueden permitirlas con `#[allow]`, y cada `allow` se justifica en la revisión.

## Procedimiento de copia (Windows/NTFS)

1. `src_hash`: `sha2::Sha256` sobre `File::open(src)` (solo lectura), en bloques de 1 MiB. Se puede reutilizar el hash del catálogo si `size` y `mtime` no cambiaron, pero la copia siempre se vuelve a verificar.
2. `dst = ensure_writable(resolve_name(dest_dir, name))?`. Si `dst` existe y tiene el mismo hash, se omite y se anota `already_present`. Si tiene otro hash, se usa `nombre (1).ext`, `nombre (2).ext`, etc.
3. Se copia con `io::copy` a `dst.part` (con `OpenOptions::create_new(true)`, que falla si ya existe) y se llama a `File::sync_all`.
4. Se calcula el SHA-256 del `.part` y debe ser igual a `src_hash`. Si no, se borra el temporal (está en el destino y pasa por el guardián), se registra el error y se sigue con el siguiente archivo.
5. Fechas, sobre el archivo abierto y antes del renombrado:
   ```rust
   use std::os::windows::fs::FileTimesExt;
   let m = fs::metadata(src)?;
   let times = FileTimes::new()
       .set_accessed(m.accessed()?).set_modified(m.modified()?).set_created(m.created()?);
   part_file.set_times(times)?;
   ```
   No se escribe nada más en el archivo después de esto.
6. `fs::rename(part, dst)`. Ambas rutas están en el destino.
7. Se registra en el manifiesto y en la tabla `export_item`.

**Qué se conserva automáticamente** con la copia byte a byte: EXIF, IPTC, XMP incrustado, perfil ICC, orientación, GPS, datos del fabricante y miniatura incrustada. **No se conservan:** ADS (`Zone.Identifier`), ACL ni propietario. Es aceptable: se declara en el informe de exportación.

**Prohibido en la exportación:** decodificar la imagen para escribirla (crates `image`, `jpeg-encoder` o `webp` en modo de escritura), convertir formatos, "normalizar orientación", quitar metadatos o comprimir. Si el usuario quiere una versión transformada, es otra función con otro nombre, siempre hacia el destino, y el original queda igual.

## Manifiesto

`manifest.csv` y `manifest.json` en la raíz del destino, escritos con el mismo patrón de archivo temporal y renombrado. Por cada archivo:
- `source_path`, `dest_path`, `sha256`, `size`, `mtime`;
- `categories[]`, `tags[]`, `entities[]` (con `type`, `name`, `role` y `confidence`);
- `status` (`copied`, `already_present` o `failed: motivo`).

Al final se añade un resumen: copiados, omitidos, fallidos y bytes.

## Sidecars XMP y etiquetas incrustadas (opcionales)

- Sidecar: `<dest_file>.xmp`, escrito solo en el destino, sin tocar ni el original ni la copia.
- Incrustar etiquetas en la copia solo si el usuario lo activa expresamente. Eso cambia los bytes de la copia: el manifiesto guarda `sha256_original` y `sha256_after_tagging`, y la escritura pasa por el guardián.

## Mover

Los originales nunca se borran. Solo `fsguard` mueve un original (`move_no_replace`) o lo manda a la Papelera (`recycle`), y solo con un `MoveSource`: un archivo normal, no un enlace ni un punto de reanálisis. El flujo de mover exige además que el catálogo lo conozca.

### Destino

`ensure_move_dir` permite carpetas dentro o fuera de una biblioteca, rechaza los datos de la app y, como siempre, no sigue enlaces que salgan de la carpeta elegida. Nunca se sobrescribe: si el nombre está ocupado por otros bytes, se usa `nombre (1).jpg`. Si ya hay un archivo idéntico (mismo SHA-256), **no se toca el original**.

### Mismo disco

Es un renombrado del sistema de archivos: atómico, sin tocar los bytes; conserva todas las fechas, los atributos y los flujos alternativos.

- Se usa `MoveFileExW` **sin** `MOVEFILE_REPLACE_EXISTING` y **sin** `MOVEFILE_COPY_ALLOWED`. `std::fs::rename` **no sirve**: en Windows reemplaza el destino si existe.
- Si Windows responde `ERROR_NOT_SAME_DEVICE`, se aplica el camino entre discos.

### Entre discos

Si algo falla, el original se queda donde estaba y solo se limpia el temporal del destino.

1. Copia verificada, como al copiar: se escribe en un `.part` en el destino, se compara su SHA-256 con el del original, se le ponen las fechas del original y se renombra dentro del destino sin sobrescribir. Al mover se copian además los atributos de solo lectura, oculto y sistema.
2. Se vuelve a comprobar el original: mismo tamaño, misma fecha de modificación y mismo SHA-256 que al empezar. Si cambió, no se toca y se informa «copiado, original conservado».
3. El original va a la Papelera con `SHFileOperationW` (`FO_DELETE`) y las banderas `FOF_ALLOWUNDO | FOF_SILENT | FOF_WANTNUKEWARNING`, como `Supr` en el Explorador: con la configuración por defecto recicla sin preguntar y **siempre pregunta** antes de borrar para siempre. **Nunca** se añaden `FOF_NOCONFIRMATION` (respondería que sí a ese borrado) ni `FOF_NOERRORUI`. Si el usuario cancela o la llamada falla, el resultado es «copiado, original conservado». Tampoco se retira (se conserva el original) lo que no está en un disco fijo según `GetDriveTypeW` de la raíz de su volumen (memorias USB y otras unidades extraíbles, red, CD) ni una ruta que la Papelera no puede recibir tal cual (demasiado larga, de red o con un nombre que Windows reinterpretaría). La Papelera **no es duradera** (Windows la vacía al llenarse o con el Sensor de almacenamiento): la protección real es la copia verificada.

No se conservan entre discos los flujos alternativos (ADS), la ACL ni el propietario; el original en la Papelera los mantiene.

### Registro y deshacer

- **Antes** de tocar un archivo se escribe su fila en `move_log` en estado `pending`. Después pasa a `done` (o `kept_original`) en la misma transacción que actualiza `location`.
- Si el movimiento falla antes de que cambie nada en el disco, la fila se descarta.
- **Al abrir el catálogo**, cada fila `pending` se resuelve mirando el disco, solo si se ven la carpeta de la biblioteca de origen y la del destino (una unidad desconectada hace que todo parezca borrado): destino presente **con el mismo SHA-256** y origen ausente → `done` (con el `dst_root` guardado en la fila); origen presente y destino ausente → se descarta; ambos presentes → `kept_original`, sin tocar nada; cualquier otro caso sigue `pending`. Un movimiento en curso mantiene un candado (una base SQLite aparte junto al catálogo, que el sistema libera aunque la app se cierre de golpe) y, mientras tanto, las filas pendientes no se tocan.
- **Deshacer** revierte, en orden inverso, el lote más reciente con filas `done`. El archivo del destino tiene que conservar el mismo SHA-256 y la ruta original tiene que estar libre; si no, se salta y se informa. Si la carpeta original no existe, se crea. `rename` vuelve con `MoveFileExW`; `copy_recycle` vuelve con el procedimiento entre discos (la copia del destino va a la Papelera). La fila pasa a `undone` y `location` vuelve a la ruta original.

## Pruebas de integridad obligatorias (`tests/safety.rs`)

- Los fixtures se generan en el test, con bytes sintéticos de JPEG y PNG que incluyen segmentos EXIF (orientación 6, GPS falso), ICC e iTXt. Nunca se usan fotos reales.
- Antes y después de indexar y exportar se guarda `sha256` + metadatos del sistema de archivos (tamaño, `mtime`, creación) de cada fixture de origen, y deben ser idénticos.
- Las copias tienen el mismo SHA-256, el mismo `mtime` y la misma fecha de creación.
- Exportar dos veces al mismo destino no crea duplicados ni sobrescribe nada.
- Al mover (siempre con una Papelera falsa inyectada, nunca la real): no se pierde ningún archivo, no se sobrescribe nada y se conservan los bytes y las fechas.
- El guardián rechaza al copiar: un destino dentro de una raíz, una raíz dentro del destino, un junction que apunta a la biblioteca y la misma ruta con otras mayúsculas.
