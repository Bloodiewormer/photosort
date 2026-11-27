# Rendimiento y escalado del catálogo

`photosort` está diseñado para catalogar bibliotecas personales de unas 35.000 imágenes manteniendo la interfaz fluida. El modo `--bench` permite medir el rendimiento completo sobre una biblioteca sintética generada por código en un directorio temporal que se elimina al terminar, sin leer ni tocar nunca archivos reales.

## Cómo ejecutar el benchmark

Compila y ejecuta siempre en modo `--release`:

```powershell
cargo run --release -p app --bin photosort -- --bench
```

Para probar otros tamaños (entre `1` y `100000`; por defecto `35000`):

```powershell
cargo run --release -p app --bin photosort -- --bench --bench-count 5000
```

Qué hace cada ejecución:

1. Genera `N` imágenes sintéticas válidas (PNG y JPEG de 16–64 px con ~3 % de duplicados exactos repartidos en ~200 subcarpetas) dentro de un directorio temporal.
2. Mide escaneo (`scanner::scan`), primera indexación con SHA-256 (`Catalog::index`), segunda indexación sin cambios (comprueba que calcula `0` hashes) y cola de reglas (`rules::run`).
3. Siembra ~25 etiquetas realistas (0–6 por imagen, combinando estados sugerido, aceptado y rechazado) mediante la API pública del catálogo.
4. Ejecuta 20 veces cada consulta interactiva (todas las ordenaciones de `entries`, filtros de 1, 2 y 3 etiquetas, filtro de texto en ruta, `labels_in_use`, `duplicate_groups`, `review_queue` y `counts`) reportando media y `p95`, además de `EXPLAIN QUERY PLAN` automático si alguna supera el objetivo.
5. Genera hasta 500 miniaturas reales de 256 px (`thumbs::Thumbs`), registra el tamaño de la biblioteca y de SQLite y lee la memoria residente pico (RSS en Windows).

## Significado de los objetivos (`PASS` / `WARN`)

| Objetivo | Umbral a 35.000 imágenes | Por qué importa |
|---|---|---|
| Segunda indexación sin cambios | `< 5 s` | Reabrir la biblioteca o comprobar archivos nuevos no debe bloquear ni releer los bytes sin cambios. |
| Consultas interactivas (`entries`, `labels_in_use`, `duplicate_groups`, `review_queue`, `counts`) | `p95 < 150 ms` | Cambiar de orden, pestaña o filtro debe responder de forma inmediata en la interfaz. |
| `entries` con 3 filtros de etiquetas (`require` + `any` + `exclude`) | `p95 < 250 ms` | Las combinaciones complejas de etiquetas siguen siendo interactivas sin pausas perceptibles. |

## Resultados medidos

**Equipo de prueba:** AMD Ryzen 5 5600G (6 núcleos / 12 hilos), 13,89 GB RAM, Windows 11, perfil `--release`.

### Escalado actual (5.000 vs 35.000 imágenes)

| Métrica | 5.000 imágenes | 35.000 imágenes | Objetivo (35k) | Estado |
|---|---:|---:|---:|:---:|
| Escaneo (`scanner::scan`) | 28,01 ms (178.485 arch/s) | 64,29 ms (544.434 arch/s) | - | INFO |
| Primera indexación (SHA-256) | 1,27 s (3.949 arch/s, 10,8 MB/s) | 12,00 s (2.916 arch/s, 8,0 MB/s) | - | INFO |
| Segunda indexación (sin cambios) | 39,84 ms (0 hash) | 365,44 ms (0 hash) | < 5 s | PASS |
| Cola de análisis (`rules::run`) | 1,24 s (3.901 arch/s) | 10,15 s (3.343 arch/s) | - | INFO |
| Miniaturas (500 reales, 256 px) | 1,03 s (488 min/s) | 512,78 ms (975 min/s) | - | INFO |
| `entries (vacío, PathAsc)` p95 | 10,56 ms | 42,93 ms | < 150 ms | PASS |
| `entries (vacío, PathDesc)` p95 | 15,16 ms | 37,36 ms | < 150 ms | PASS |
| `entries (vacío, SizeAsc)` p95 | 17,78 ms | 39,99 ms | < 150 ms | PASS |
| `entries (vacío, SizeDesc)` p95 | 53,52 ms | 41,49 ms | < 150 ms | PASS |
| `entries (vacío, ModifiedAsc)` p95 | 38,45 ms | 44,17 ms | < 150 ms | PASS |
| `entries (vacío, ModifiedDesc)` p95 | 24,95 ms | 39,09 ms | < 150 ms | PASS |
| `entries (1 etiqueta: require)` p95 | 15,13 ms | 16,80 ms | < 150 ms | PASS |
| `entries (2 etiquetas: req + any)` p95 | 9,79 ms | 13,08 ms | < 150 ms | PASS |
| `entries (3 etiquetas: req+any+excl)` p95 | 10,06 ms | 21,81 ms | < 250 ms | PASS |
| `entries (filtro de texto en ruta)` p95 | 12,18 ms | 19,94 ms | < 150 ms | PASS |
| `labels_in_use` p95 | 6,38 ms | 14,55 ms | < 150 ms | PASS |
| `duplicate_groups` p95 | 3,95 ms | 5,26 ms | < 150 ms | PASS |
| `review_queue` p95 | 18,45 ms | 29,72 ms | < 150 ms | PASS |
| `counts` p95 | 0,33 ms | 1,28 ms | < 150 ms | PASS |
| Tamaño de biblioteca / BD | 13,67 MB / 9,03 MB | 95,71 MB / 37,28 MB | - | - |
| Memoria residente pico (RSS) | 24,68 MB | 92,98 MB | - | - |

## Qué se optimizó y comparación antes / después (35.000 imágenes)

En la medición inicial con 35.000 imágenes, la **segunda indexación sin cambios** tardó **50,26 s (`WARN`)**, la primera indexación **204,56 s** y `rules::run` **140,77 s**. Además, `EXPLAIN QUERY PLAN` mostró recorridos completos y árboles temporales (`USE TEMP B-TREE`) en `labels_in_use`, `review_queue`, `duplicate_groups` y `counts`.

Cambios aplicados en `crates/catalog` sin alterar el comportamiento:

1. **Transacciones por lotes y sentencias cacheadas en `Catalog::index_with` (`crates/catalog/src/lib.rs`)**: agrupa hasta 512 archivos por transacción usando `prepare_cached` y confirma inmediatamente al recibir `Control::Stop`, evitando 35.000 commits individuales.
2. **Pragmas de SQLite para modo WAL (`crates/catalog/src/lib.rs`)**: `PRAGMA synchronous = NORMAL` (recomendado por SQLite en modo WAL), `PRAGMA temp_store = MEMORY` y `PRAGMA cache_size = -32000` (32 MB de caché de páginas para evitar expulsiones de caché en 35k filas).
3. **Índices compactos de enteros y parciales (`crates/catalog/src/lib.rs`)**:
   - `location_missing_asset ON location(missing, asset_id)` y `location_asset_missing ON location(asset_id, missing)` (cobertura sin duplicar cadenas `path` en disco).
   - `asset_label_active_label ON asset_label(label_id, asset_id) WHERE status <> 'rejected'` y `UNIQUE asset_label_active_asset ON asset_label(asset_id, label_id) WHERE status <> 'rejected'`.
   - `asset_label_review ON asset_label(confidence, asset_id, label_id) WHERE status = 'suggested'`.
4. **Ajuste de consultas (`crates/catalog/src/browse.rs` y `crates/catalog/src/labels.rs`)**:
   - `labels_in_use`: elimina `COUNT(DISTINCT al.asset_id)` redundante (la clave primaria `(asset_id, label_id)` ya garantiza unicidad) y agrupa por `al.label_id` en flujo sobre `asset_label_active_label` sin `TEMP B-TREE FOR GROUP BY`.
   - `review_queue`: elimina el `EXISTS` duplicado sobre `location`, calculando `MIN(path)` una sola vez y descartando filas sin ubicación presente.

| Operación (35.000 imágenes) | Antes | Después | Mejora |
|---|---:|---:|---:|
| Segunda indexación (sin cambios) | 50,26 s (`WARN`) | 365,44 ms (`PASS`) | 137,5x |
| Primera indexación (SHA-256) | 204,56 s | 12,00 s | 17,0x |
| Cola de análisis (`rules::run`) | 140,77 s | 10,15 s | 13,9x |
| `labels_in_use` (p95) | 92,35 ms | 14,55 ms | 6,3x |
| `review_queue` (p95) | 67,54 ms | 29,72 ms | 2,3x |
| `duplicate_groups` (p95) | 26,12 ms | 5,26 ms | 5,0x |
| `counts` (p95) | 11,27 ms | 1,28 ms | 8,8x |
| `entries (3 etiquetas)` (p95) | 53,57 ms | 21,81 ms | 2,5x |
| `entries (SizeAsc)` (p95) | 72,81 ms | 39,99 ms | 1,8x |
| `entries (ModifiedAsc)` (p95) | 70,17 ms | 44,17 ms | 1,6x |

### Duplicados parecidos (todos contra todos sobre embeddings SigLIP 2)

Medido con la prueba `#[ignore]` `benchmark_near_duplicates_45k_embeddings` (`cargo test --release -p catalog --test performance benchmark_near_duplicates_45k_embeddings -- --ignored --nocapture`) en el mismo equipo de prueba:

- **45.000 vectores sintéticos de 768 dimensiones** (~1.012 millones de pares), umbral `0,95`, cálculo en paralelo con `std::thread::scope` y mosaicos de 64 × 64: **39,07 s** (detectando los 10 grupos inyectados).
- El resultado se guarda en las tablas `near_duplicate_group` y `near_duplicate_member` para no recalcular al abrir la aplicación.
