# Copiar o mover fotos: plan de implementación

> **Para agentes:** sigue las tareas en orden; cada paso usa casillas (`- [ ]`).

**Objetivo:** que la ventana «Copiar» ofrezca Copiar o Mover (cortar) de forma verificada y reversible, con
Recargar y Deshacer en la galería.

**Arquitectura:**
- `fsguard` es el único que mueve un original o lo manda a la Papelera.
- `export` reutiliza la copia verificada actual y suma el modo mover.
- `catalog` guarda el registro de movimientos y actualiza `location`.
- `app` cambia la ventana y añade los botones.

**Tecnología:** Rust 2024, egui 0.37, rusqlite y windows-sys 0.61.2 (con las features nuevas
`Win32_Storage_FileSystem` y `Win32_UI_Shell` en `fsguard`).

**Diseño:** `docs/superpowers/specs/2026-11-27-mover-fotos-design.md`. Léelo entero antes de empezar.

## Restricciones globales

- **Nunca borrar.** El único camino para retirar un original es `fsguard::recycle`, que lo manda a la
  Papelera (`FOF_ALLOWUNDO`) y solo después de una copia verificada.
- **Nunca sobrescribir.** `std::fs::rename` sigue prohibido para mover originales, porque en Windows reemplaza el
  destino. Se usa `MoveFileExW(src, dst, 0)`.
- **Nunca volver a codificar** ni cambiar bytes, fechas o atributos de un archivo movido.
- **Datos para las pruebas.** Solo fixtures sintéticos en `tempfile::TempDir`. Nada en `D:`. Ningún test
  llama a la Papelera real: se usa el `retire` inyectado.
- **`clippy.toml`.** Se conservan todas las prohibiciones. Solo `fsguard` y `export` llevan
  `#[allow(clippy::disallowed_methods)]`, cada uno con su comentario.
- **Commits.** En español, en imperativo, sin menciones de herramientas de IA ni coautores (`CONTRIBUTING.md`).
  Rama `feat/copiar-mover`, que sale de `master`.
- **Verificación por tarea.** `cargo fmt --check`, `cargo clippy -p <crate> --all-targets -- -D warnings` y los
  tests de la crate tocada.

## Riesgos que revisar

1. **El índice corre mientras se mueve.** La revisión de carpetas puede insertar ya una fila `location` para el
   destino (que es `UNIQUE`). `finish_move` no debe fallar: borra la fila del origen en lugar de actualizarla.
   Test en la tarea 3.
2. **El original cambia o desaparece entre la vista previa y el movimiento.** El resultado es
   `Failed`/`KeptOriginal`, nunca una retirada. Test en la tarea 4.
3. **El mismo archivo seleccionado dos veces, o dos fotos con el mismo nombre que van al mismo destino.** La
   segunda recibe `nombre (1)`. Ningún original se retira sin su copia. Test en la tarea 4.
4. **Deshacer con la ruta original ocupada o con la copia del destino modificada.** Esa fila se salta, se
   informa y no se toca ningún archivo. Test en la tarea 4.
5. **Cancelar a mitad de un lote.** Lo ya movido queda `done` y se puede deshacer; lo que falta no se toca.
   Test en la tarea 5.

---

### Tarea 1: Reglas y documentos

**Archivos:**
- Modificar: `AGENTS.md` (resumen innegociable).
- Modificar: `.claude/skills/media-library-catalog/SKILL.md` (§1, la fila «Exportación» de §4 y §5).
- Modificar: `.claude/skills/media-library-catalog/references/integrity.md` (sección nueva «Mover»).
- Modificar: `docs/propuesta-tecnica.md` (donde diga solo lectura) y `docs/decisiones.md` (entrada
  2026-11-27).
- Modificar: `clippy.toml` (comentario de cabecera).

- [ ] **Paso 1.** Reescribir la regla: los originales no se borran, no se vuelven a codificar, no se
  sobrescriben y no se les cambian metadatos ni fechas. Solo se mueven por acción expresa del usuario.
  - En el mismo disco, con `MoveFileExW` sin reemplazar.
  - Entre discos: copia verificada, comprobar de nuevo el original y mandarlo a la Papelera.
  - Siempre con registro y con deshacer.
  - Ningún otro código mueve: solo `fsguard::move_no_replace` y `fsguard::recycle`.
  - Mantener la prohibición de crear archivos junto a los originales, con la excepción del destino de un
    movimiento elegido por el usuario (y su `.xmp` opcional).
- [ ] **Paso 2.** En `integrity.md`, copiar el procedimiento exacto del diseño (secciones «Mismo disco» y
  «Entre discos», y el registro con deshacer).
- [ ] **Paso 3.** En `decisiones.md`, añadir: «**2026-11-27 · La app puede mover fotos, nunca borrarlas**
  (opción A: copia verificada y Papelera entre discos). Alternativas: dejar el original oculto (no cumple que
  dejen de estar en D:) o retirarlo para siempre (sin vuelta atrás). Pedido por el usuario para separar y
  ordenar.»
- [ ] **Paso 4.** Hacer `git grep -n -i "solo lectura\|read-only\|nunca se mueven\|never moved"` y ajustar cada
  frase que contradiga la regla nueva. Los comentarios de código se ajustan en la tarea que toca ese código.
- [ ] **Paso 5.** Commit: `Permite mover fotos sin borrarlas y documenta cómo se protege el original`.

### Tarea 2: `fsguard` aprende a mover

**Archivos:**
- Modificar: `crates/fsguard/Cargo.toml`. Añadir la dependencia
  `[target."cfg(windows)".dependencies] windows-sys = { version = "0.61.2", features = ["Win32_Storage_FileSystem", "Win32_UI_Shell", "Win32_Foundation"] }`.
- Modificar: `crates/fsguard/src/lib.rs`.
- Test: `crates/fsguard/tests/moves.rs` (nuevo).

**Interfaces que produce:**
- `pub struct MoveSource` (ruta canónica de un archivo normal que no es enlace), con `pub fn as_path(&self) -> &Path`.
- `pub fn ensure_move_source(path: &Path) -> Result<MoveSource, GuardError>`. Rechaza `..`, las rutas que no
  existen, las carpetas y los enlaces o puntos de reanálisis (`symlink_metadata().file_type().is_symlink()`;
  en Windows también `FILE_ATTRIBUTE_REPARSE_POINT`).
- `pub fn ensure_move_dir(path: &Path, app_data: &Path) -> Result<DestPath, GuardError>`. Devuelve un
  `DestPath` cuyo `roots` es `[canonical(app_data)]`. Así, `verify` y `join_name` rechazan entrar en los datos de
  la app, pero sí permiten bibliotecas. Si la ruta está dentro de `app_data`, devuelve
  `GuardError::InsideAppData { path }` (variante nueva, mensaje: «la carpeta está dentro de los datos de
  photosort; elige otra»).
- `pub fn move_no_replace(src: &MoveSource, dst: &DestPath) -> io::Result<()>`.
  - Windows: `MoveFileExW` con flags `0`. Llama a `dst.verify()` justo antes.
  - Error 17 (`ERROR_NOT_SAME_DEVICE`) devuelve `io::ErrorKind::CrossesDevices`; 80 o 183 (ya existe)
    devuelve `AlreadyExists`.
  - Fuera de Windows: falla con `AlreadyExists` si `dst` existe; si no, usa `std::fs::rename`, con la
    anotación `shortcut: comprobar y renombrar no es atómico fuera de Windows, solo afecta al CI de Linux`.
- `pub fn recycle(file: &MoveSource) -> io::Result<bool>`.
  - Windows: `SHFileOperationW` con `FO_DELETE` y
    `FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING`.
  - `pFrom` es la ruta sin el prefijo `\\?\` (usa `dunce::simplified`) y termina en doble NUL.
  - Devuelve `Ok(true)` solo si la llamada devolvió 0, `fAnyOperationsAborted == 0` y el archivo ya no existe.
    `Ok(false)` significa que el usuario canceló el aviso de borrado definitivo o que el archivo sigue ahí.
  - Fuera de Windows: `Ok(false)`, para que nunca se retire nada.
- Comentario en el `#[allow]`: «único lugar que mueve o manda a la Papelera un original; nunca borra».

- [ ] **Paso 1: escribir los tests que fallan** en `crates/fsguard/tests/moves.rs`:
  - `move_source_refuses_folders_links_and_missing_files`;
  - `move_dir_allows_a_library_but_refuses_app_data` (incluye una junction hacia `app_data` creada con
    `junction::create` si la crate ya está en el workspace; si no, solo la ruta directa);
  - `same_volume_move_keeps_bytes_and_all_dates`. Compara el SHA-256 y `created()`, `modified()` y
    `accessed()` antes y después; las fechas se fijan antes con `File::set_times` a valores de 2001;
  - `move_never_replaces_an_existing_file`. El destino existe con otros bytes y `move_no_replace` devuelve
    `AlreadyExists`. Los dos archivos quedan intactos;
  - **no** hay test de `recycle` real.
- [ ] **Paso 2.** `cargo test -p fsguard --test moves`: debe fallar porque las funciones no existen.
- [ ] **Paso 3.** Implementar las interfaces de arriba.
- [ ] **Paso 4.** `cargo test -p fsguard` en verde y `cargo clippy -p fsguard --all-targets -- -D warnings`.
- [ ] **Paso 5.** Commit: `Añade al guardián el movimiento sin reemplazo y el envío a la Papelera`.

### Tarea 3: Registro de movimientos en el catálogo

**Archivos:**
- Modificar: `crates/catalog/src/lib.rs` (esquema y llamada en `open`).
- Crear: `crates/catalog/src/moves.rs` (`mod moves;`).
- Test: `crates/catalog/tests/moves.rs`.

**Interfaces que produce:**

```rust
pub enum MoveMethod { Rename, CopyRecycle }          // texto en la base: 'rename' | 'copy_recycle'
pub enum MoveState { Pending, Done, KeptOriginal, Undone }
pub struct NewMove<'a> { pub batch: i64, pub src_path: &'a Path, pub src_root: &'a Path,
    pub dst_path: &'a Path, pub sha256: &'a str, pub size: u64, pub mtime_ns: Option<i64>,
    pub method: MoveMethod }
pub struct MoveRow { pub id: i64, pub batch: i64, pub src_path: PathBuf, pub src_root: PathBuf,
    pub dst_path: PathBuf, pub sha256: String, pub size: u64, pub mtime_ns: Option<i64>,
    pub method: MoveMethod }
impl Catalog {
    pub fn next_move_batch(&self) -> Result<i64, CatalogError>;
    pub fn begin_move(&self, m: &NewMove) -> Result<i64, CatalogError>;          // fila en 'pending'
    pub fn finish_move(&self, id: i64, state: MoveState, dst_root: Option<&Path>) -> Result<(), CatalogError>;
    pub fn last_undoable_batch(&self) -> Result<Option<Vec<MoveRow>>, CatalogError>; // filas 'done', id DESC
    pub fn set_move_method(&self, id: i64, method: MoveMethod) -> Result<(), CatalogError>; // antes de copiar
    pub fn undo_done(&self, id: i64) -> Result<(), CatalogError>;
    pub(crate) fn resolve_pending_moves(&self) -> Result<(), CatalogError>;
}
```

- Tabla `move_log`: las columnas del diseño, más `src_root TEXT NOT NULL`. Índice en `(state, batch)`.
- `finish_move(id, Done, Some(root))` cambia `path` y `root` de la fila `location` del origen, en la misma
  transacción. Si ya existe una fila con `path = dst`, borra la del origen.
- `finish_move(id, Done, None)` borra la fila `location` del origen. No toca ni `asset` ni `asset_label`.
- `undo_done` pasa la fila a `undone` y deja `location` en la ruta original:
  - si existe una fila con el destino, la actualiza;
  - si no, inserta una con `asset_id` por SHA-256, `root = src_root`, `seen_run = MAX(seen_run)` y
    `missing = 0`.
- `resolve_pending_moves` aplica las reglas del diseño mirando `Path::exists`. Se llama en `Catalog::open`,
  después de las calibraciones.

- [ ] **Paso 1: escribir los tests que fallan:**
  - `a_finished_move_inside_a_library_keeps_the_labels_on_the_new_path`;
  - `a_move_out_of_the_libraries_hides_the_photo_but_keeps_its_labels` (`entries` no la incluye;
    `labels_of(asset)` sigue igual);
  - `finishing_a_move_survives_an_index_that_already_saw_the_destination`;
  - `undo_brings_the_photo_back_to_its_original_path`;
  - `pending_moves_are_resolved_when_the_catalog_opens` (tres casos: solo destino, solo origen, ambos);
  - `the_last_undoable_batch_steps_back_one_batch_at_a_time`.
- [ ] **Paso 2.** `cargo test -p catalog --test moves`: debe fallar.
- [ ] **Paso 3.** Implementar.
- [ ] **Paso 4.** `cargo test -p catalog` y clippy en verde.
- [ ] **Paso 5.** Commit: `Registra en el catálogo cada movimiento para poder deshacerlo`.

### Tarea 4: Mover y deshacer en `export`

**Archivos:**
- Modificar: `crates/export/src/lib.rs`.
- Test: `crates/export/tests/moves.rs` (nuevo).

**Consume:** las tareas 2 y 3.

**Interfaces que produce:**
- `#[derive(Default)] pub enum Operation { #[default] Copy, Move }` y un campo `pub operation: Operation` en
  `ExportOptions`. Hay que actualizar las construcciones literales que fallen al compilar.
- Nuevas variantes de `Outcome`:
  - `Moved(PathBuf)`;
  - `KeptOriginal { dest: PathBuf, reason: String }`: copiado, pero el original sigue ahí. Lo produce un
    original que cambió, una Papelera cancelada o un fallo de la Papelera.
- ```rust
  pub struct MoveContext<'a> {
      pub catalog: &'a catalog::Catalog,
      pub roots: &'a [fsguard::LibraryRoot],
      /// `fsguard::recycle` in the app; a stand-in that moves to a temp folder in tests.
      pub retire: &'a dyn Fn(&fsguard::MoveSource) -> io::Result<bool>,
      /// Tests only: take the other-disk path on the same disk.
      pub force_cross_volume: bool,
      /// Tests only: runs after the verified copy and before the original is checked again.
      pub before_retire: Option<&'a dyn Fn(&Path)>,
  }
  ```
- `pub fn move_with_options(items: &[ExportItem], dest: &DestPath, options: ExportOptions, ctx: &MoveContext, progress: &mut dyn FnMut(&ExportProgress) -> Control) -> io::Result<ExportReport>`.
  - Comparte el bucle y el manifiesto con `export_with_options`: extrae una función interna
    `run(items, dest, options, mover: Option<&MoveContext>, progress)`.
  - Ignora `embed_jpeg_tags`.
  - Un lote equivale a una llamada: `ctx.catalog.next_move_batch()`.
- Lo que hace `move_with_options` con cada archivo:
  1. Lee el original: `read_source` da `md` y `sha`.
  2. Resuelve el nombre en el destino sin sobrescribir; con `resolve` existente, si es igual (`same`), da
     `AlreadyPresent` y no toca nada.
  3. Llama a `begin_move` con el método `Rename`.
  4. Llama a `move_no_replace`, salvo que `force_cross_volume` esté activo. Si va bien, `finish_move(Done)` con
     la raíz de destino; esa raíz se busca en `ctx.roots` con la misma comparación sin mayúsculas que usa
     `fsguard`. Resultado: `Moved`.
  5. Si devuelve `CrossesDevices` (o `force_cross_volume`):
     - `set_move_method(id, CopyRecycle)` antes de escribir nada (renombrar no tocó el disco);
     - `write_verified` al destino;
     - si `ctx.before_retire` existe, se llama con la ruta del original (gancho para los tests);
     - vuelve a leer el original y compara su tamaño, `mtime` y SHA-256 con `md` y `sha`;
     - si coinciden, `retire(src)`. Si devuelve `true`, `finish_move(Done)` y `Moved`; en cualquier otro caso,
       `finish_move(KeptOriginal)` y `KeptOriginal`.
  6. Cualquier error antes de retirar el original termina en `finish_move(KeptOriginal)` si la copia existe;
     si no existe, el resultado es `Failed`, y la fila `pending` se resuelve en el siguiente `open`.
- El manifiesto lleva la columna nueva `operacion` (`copiar` o `mover`) en el CSV y en el JSON. Estados nuevos:
  `moved` y `kept_original: <motivo>`.
- `Preview` gana `pub same_volume: usize` y `pub other_volume: usize`.
  - Se calculan comparando el prefijo de unidad de las rutas canónicas, con la anotación
    `shortcut: un punto de montaje o subst cuenta como el disco de su letra; mover cae solo al camino entre discos`.
  - En modo mover, `fits()` solo cuenta los bytes de `other_volume`. Agregar el campo
    `pub bytes_other_volume: u64`.
- `pub struct UndoReport { pub restored: Vec<PathBuf>, pub skipped: Vec<(PathBuf, String)> }`.
- `pub fn undo_last_move(ctx: &MoveContext, app_data: &Path) -> io::Result<Option<UndoReport>>`.
  - Toma `last_undoable_batch`. Por cada fila:
    1. Exige que `src_path` no exista y que el SHA-256 de `dst_path` sea igual a `sha256`. Si no, la salta con
       su motivo.
    2. Crea la carpeta padre de `src_path` con `ensure_move_dir(parent, app_data)` y `create_dir_all`.
    3. `Rename`: `move_no_replace(ensure_move_source(dst), DestPath de src)`.
    4. `CopyRecycle`: `write_verified(dst → src)` y después `retire(dst)`. Si `retire` devuelve `false`, el
       original igual está restaurado, solo se avisa.
    5. `undo_done(id)`.
  - Devuelve `None` si no hay nada que deshacer.

- [ ] **Paso 1: escribir los tests que fallan** en `crates/export/tests/moves.rs`, con un catálogo real en
  `TempDir` indexado sobre una raíz sintética:
  - `moving_on_the_same_disk_keeps_bytes_dates_and_labels`;
  - `moving_to_another_disk_retires_the_original_only_after_a_verified_copy`. Usa `force_cross_volume` y un
    `retire` que mueve a `tmp/papelera`; comprueba que la copia tiene el SHA-256 y que el original está en la
    papelera falsa;
  - `a_declined_recycle_keeps_the_original` (`retire` devuelve `Ok(false)`, el resultado es `KeptOriginal`);
  - `an_original_changed_during_the_copy_is_never_retired`. `before_retire` reescribe el original; el
    `retire` falso entra en pánico si se le llama y el resultado es `KeptOriginal`;
  - `moving_never_overwrites_and_an_identical_file_keeps_the_original`;
  - `two_sources_with_the_same_name_get_numbered_and_none_is_lost`;
  - `undo_restores_identical_files_and_skips_an_occupied_path`;
  - `the_manifest_says_whether_it_copied_or_moved`.
- [ ] **Paso 2.** `cargo test -p export --test moves`: debe fallar.
- [ ] **Paso 3.** Implementar.
- [ ] **Paso 4.** `cargo test -p export` (también los tests existentes de copia) y clippy en verde.
- [ ] **Paso 5.** Commit: `Mueve fotos con verificación y permite deshacer el último movimiento`.

### Tarea 5: La ventana «Copiar o mover»

**Archivos:**
- Modificar: `crates/app/src/export_flow.rs`, `crates/app/src/views/export.rs` y `crates/app/src/ui.rs`
  (donde se abre la ventana y se procesa su fin).
- Test: `crates/app/tests/export_flow.rs` y `crates/app/tests/safety.rs`.

**Consume:** la tarea 4.

**Lo que cambia:**
- `ExportWizard` gana `operation: Operation` con `set_operation` y `operation()`.
  - En modo mover, `embed_jpeg_tags` queda en falso y la casilla se desactiva con la nota «Al mover no se
    incrustan etiquetas: cambiaría el único ejemplar».
- `validate_destination(path, roots, operation, app_data)`:
  - copiar usa `ensure_export_dir`, como hoy;
  - mover usa `ensure_move_dir`, y además rechaza que el destino sea exactamente la carpeta de cada origen.
    Si todos los orígenes están ya ahí, el mensaje es «Las fotos ya están en esa carpeta».
- Fase nueva `ExportPhase::ConfirmMove`, entre `Preview` listo y `Running`. Texto exacto:
  «Se moverán {n} fotos fuera de sus carpetas. {m} irán a la Papelera después de copiarse y verificarse.
  Puedes deshacerlo.» Botones **Mover** y **Volver**.
- `start_export(notify, db: DestPath, roots: Vec<LibraryRoot>, app_data: PathBuf)`. En modo mover, el hilo abre
  su propio `Catalog::open(&db)` y llama a `move_with_options`, con `retire = &fsguard::recycle`.
- `ExportSummary` cuenta `moved` y `kept_original` y expone `operation`.
- Textos:
  - título de la ventana: «Copiar o mover»;
  - selector «Copiar | Mover (cortar)» arriba del todo;
  - el botón final dice «Copiar» o «Mover…»;
  - la vista previa muestra «{a} en el mismo disco (instantáneo) · {b} a otro disco (copia y Papelera)».
- Al terminar un movimiento, `ui.rs` recarga las entradas de la galería: el mismo camino que usa tras
  `mark_gone_photos`.

- [ ] **Paso 1: escribir los tests que fallan:**
  - `export_flow.rs`: `move_mode_asks_for_a_second_confirmation_before_touching_anything` (desde
    `Preview` listo, `start_export` en modo mover sin confirmar da error y el origen sigue ahí);
  - `export_flow.rs`: `move_mode_turns_off_tag_embedding`;
  - `export_flow.rs`: `cancelling_a_move_keeps_what_was_moved_undoable_and_leaves_the_rest`;
  - `safety.rs`: `copy_mode_still_leaves_the_originals_untouched` (renombrar el test de exportación si hace
    falta, sin debilitarlo);
  - `safety.rs`: `move_mode_only_moves_and_never_deletes`. Recuento antes y después: archivos en el origen
    más archivos en el destino más la papelera falsa igual al recuento inicial, y cada SHA-256 aparece una
    vez fuera de la papelera.
- [ ] **Paso 2.** Ejecutar esos tests: deben fallar.
- [ ] **Paso 3.** Implementar.
- [ ] **Paso 4.** `cargo test -p app --test export_flow --test safety` y clippy en verde.
- [ ] **Paso 5.** Commit: `Convierte la exportación en la ventana Copiar o mover`.

### Tarea 6: Recargar y Deshacer en la galería

**Archivos:**
- Modificar: `crates/app/src/views/gallery.rs` (barra superior y barra de selección),
  `crates/app/src/views/context_menu.rs` («Exportar…» pasa a «Copiar o mover…») y `crates/app/src/ui.rs`.
- Modificar: `docs/uso.md` (la sección «Exportar» pasa a «Copiar o mover»; Recargar; Deshacer) y la tabla de
  `docs/diseno.md`.
- Test: `crates/app/tests/missing_photos.rs` y `crates/app/tests/accessibility.rs`.

**Lo que cambia:**
- Botón **Recargar** en la barra de la galería.
  - Llama a `review_engine.request_index()`.
  - Mientras `engine_snapshot.phase` sea `Indexing`, se muestra desactivado como «Recargando…».
  - Al terminar, la galería se recarga como ya lo hace tras cada índice.
- Botón **Deshacer último movimiento**.
  - Se activa cuando `catalog.last_undoable_batch()` es `Some`. Se consulta al abrir la app y después de cada
    movimiento o deshacer, no en cada frame.
  - Corre `export::undo_last_move` en un hilo, con el mismo patrón que `ExportWorker`.
  - Al terminar, muestra «Se restauraron {n} fotos» y, si hubo omitidas, «{k} no se pudieron restaurar: …», y
    recarga la galería.
- El resumen final de un movimiento ofrece también **Deshacer este movimiento**, que hace lo mismo.
- La barra de selección y el menú contextual dicen «Copiar o mover…».

- [ ] **Paso 1: escribir los tests que fallan:**
  - `missing_photos.rs`: `reload_removes_photos_moved_outside_the_app` (un fixture se mueve con `fs::rename`
    en el test; tras pulsar Recargar y esperar al índice, la galería ya no lo muestra);
  - `accessibility.rs`: los botones nuevos tienen nombre accesible y se alcanzan con `Tab`.
- [ ] **Paso 2.** Ejecutarlos: deben fallar.
- [ ] **Paso 3.** Implementar y actualizar `uso.md` y `diseno.md`.
- [ ] **Paso 4.** `cargo test -p app` completo, `cargo clippy --workspace --all-targets -- -D warnings` y
  `cargo fmt --check`.
- [ ] **Paso 5.** Commit: `Añade Recargar y Deshacer último movimiento a la galería`.
- [ ] **Paso 6.** Abrir un PR desde `feat/copiar-mover` contra `master`. Revisar el título y el cuerpo con grep
  por palabras prohibidas antes de crearlo.
