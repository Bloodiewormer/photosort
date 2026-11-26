# Evaluación de visión local (2026-11-26)

La crate `crates/vision` evalúa **SigLIP 2** (`base-patch16-256`, embeddings de imagen y texto) y **YuNet** (detección de rostros) **antes** de integrarlos en la app o en el catálogo. Todas las pruebas de recuperación y rendimiento se hicieron con **imágenes sintéticas generadas en código**. No se leyó ninguna foto del usuario ni se accedió a la biblioteca ni a ninguna carpeta fuera de `D:\photosort\models`. **Estas cifras verifican que el pipeline funciona correctamente y miden velocidad/memoria en este equipo; no dicen nada sobre la precisión en fotos reales, manga real ni rostros reales.**

Equipo: Windows 11, Ryzen 5 5600G (6 núcleos / 12 hilos), GTX 1650 SUPER de 4 GB (~1.260 MiB ya ocupados por otros programas), 14 GB de RAM.

## Resumen

- **Entorno ONNX Runtime compartido:** `ocr::engine::init_runtime` construye una única instancia de `ort::environment::Environment` por proceso (telemetría desactivada, DirectML registrado con `error_on_failure`) y la entrega tanto a `oar-ocr` como a `crates/vision`. Ambos órdenes de carga (`ocr → vision` y `vision → ocr`) funcionan y ambos rechazan cambiar de proveedor dentro del mismo proceso.
- **DirectML verificado sin caída silenciosa:** pedir el dispositivo 99 falla en `dml_provider_factory.cc` («no adapter with the specified ordinal»). En `dml:0`, SigLIP 2 visión coloca 463/466 nodos en DirectML, SigLIP 2 texto coloca 418/480 en DirectML y YuNet coloca todos sus nodos en DirectML.
- **Acuerdo numérico CPU frente a DirectML:**
  - SigLIP 2 visión (`fp16`, 20 imágenes): similitud coseno mediana **0,999991** (mínima 0,999968, media 0,999990, `max |diff| = 0,001227`, 0 valores no finitos).
  - SigLIP 2 texto (`int8`, 40 prompts EN+ES): DirectML lo ejecuta correctamente; coseno mediana **0,999139** (mínima 0,998771, media 0,999115, `max |diff| = 0,010122`, 0 valores no finitos).
- **Recomendación medida en este equipo:**
  - **SigLIP 2 visión:** **DirectML con lote 8** (**95,4 ms/imagen**, 10,5 img/s; 1,88× más rápido que la mejor CPU de 178,9 ms/imagen con lote 1; +335 MiB de VRAM frente a +457 MiB con lote 16 para el mismo rendimiento de 10,5 img/s). Si el proceso corre en CPU, usar **hilos por defecto y lote 1** (los lotes 8 y 16 empeoran latencia y duplican la RAM en CPU).
  - **SigLIP 2 texto (`int8`):** para consultas individuales (lote 1), **gana la CPU** (17,4 ms frente a 28,8 ms en DirectML, por los 62 nodos de `Gather`/`Unsqueeze`/`Concat` que ORT deja en CPU). En lote 16 (p. ej. al calcular un banco fijo de etiquetas), DirectML alcanza 17,9 ms/prompt (55,7/s) frente a 24,9 ms/prompt en CPU. Dado que el proceso comparte un único proveedor, en modo DirectML una consulta de texto tarda ~29 ms (instantánea en interacción) y un lote de 16 prompts ~287 ms.
  - **YuNet:** **2,66 ms/imagen** en DirectML (+25 MiB de VRAM) frente a **3,44 ms/imagen** en CPU (hilos por defecto). El cuello de botella real es el preprocesado (letterbox a 640×640 + BGR: ~7,5 ms/imagen).

## Versiones, archivos y licencias

### Crates

`crates/vision` reutiliza `ort = "=2.0.0-rc.15"` (con feature `directml`), `image`, `scanner` y `ocr` ya presentes en el workspace, y declara `tokenizers = "=0.23.2"` con `default-features = false` (sin `http`, sin `hf-hub`, sin `progressbar`).

| Crate | Versión | SHA-256 en `Cargo.lock` | Licencia | Notas |
|---|---|---|---|---|
| `tokenizers` | =0.23.2 | `7afbf6e88718afcc138bad01d6ccc3051dbbc3b2ce9793d8b8a3aeb610969cfc` | Apache-2.0 | Ya estaba en `Cargo.lock` vía `oar-ocr-core`. `crates/vision` usa `default-features = false`. |
| `ort` | =2.0.0-rc.15 | `37c87aa7a90b7ff3e0fe36cccd475df8d67f15f07f4d6c370c4b38464232813c` | MIT o Apache-2.0 | Misma instancia que `crates/ocr`; enlace estático a `onnxruntime.lib` (`rel-1.31.0`, commit `402979a`). |

**Dependencias nuevas en `Cargo.lock`:** ninguna crate externa nueva ni nueva crate que compile C (el único cambio en `Cargo.lock` es la entrada del paquete local `vision`). Nota: `onig_sys` (C) ya existía en `Cargo.lock` por `oar-ocr-core` → `tokenizers/onig`, no fue introducida por `vision`.

### Modelos fijados (sin descarga automática)

El motor verifica tamaño en bytes y SHA-256 de todos los archivos antes de cargar ninguna sesión o tokenizador, y rechaza cargar si falta alguno o difiere un solo byte.

| Archivo (bajo raíz de modelos) | Bytes | SHA-256 | Origen | Licencia |
|---|---:|---|---|---|
| `siglip2/vision_model_fp16.onnx` | 186.131.676 | `fe9ad8020a6d3d98d394c9be8f07064066135fc2f87ec11692de0b677c0ac4db` | `onnx-community/siglip2-base-patch16-256-ONNX` rev `d1114256522a37ffa257a0a58017348ab0058db2` (original `google/siglip2-base-patch16-256`) | Apache-2.0 |
| `siglip2/text_model_int8.onnx` | 283.438.275 | `6f59b39d880c413042314b79302b74d0dd93b273caf8fbfdb1eb2df61a7fefd4` | idem | Apache-2.0 |
| `siglip2/tokenizer.json` | 34.363.039 | `cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322` | idem | Apache-2.0 |
| `siglip2/config.json` | 458 | `795a8e216ebb51ad5043e128e557c45279b686bd672221fbe01279a7b8cb79c9` | idem | Apache-2.0 |
| `siglip2/preprocessor_config.json` | 394 | `d14ba2ee3fd816f3de8abaddc31953565128eaf37c73ad4bed32101a98465aff` | idem | Apache-2.0 |
| `siglip2/tokenizer_config.json` | 47.240 | `7c3a247599e741bceba1a3fe0285aea88d1044dc1fad2caa1e48cdd9fd25f630` | idem | Apache-2.0 |
| `yunet/face_detection_yunet_2023mar.onnx` | 232.589 | `8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4` | `opencv/face_detection_yunet` rev `3cc26e7f1014a5ee5d74a42acee58bafc9d0a310` | MIT |

## Inspección real de los modelos ONNX

Inspeccionados directamente con `vision_eval inspect` y comprobando la ausencia de `logit_scale`/`logit_bias` en los binarios:

- **`siglip2/vision_model_fp16.onnx`:** entrada `pixel_values` **`Float32`** `[batch, 3, 256, 256]` (el cast a fp16 es interno al grafo); salidas `last_hidden_state` `Float32` `[-1, 256, 768]` y `pooler_output` `Float32` `[-1, 768]`. Se usa `pooler_output` y se normaliza L2.
- **`siglip2/text_model_int8.onnx`:** entrada `input_ids` **`Int64`** `[batch, seq_len]`; salidas `last_hidden_state` `Float32` `[-1, -1, 768]` y `pooler_output` `Float32` `[-1, 768]`. Se usa `pooler_output` y se normaliza L2.
- **Sin `logit_scale` ni `logit_bias`:** comprobado que ninguno de los dos archivos ONNX contiene `logit_scale` ni `logit_bias`. La crate **no inventa probabilidades** y expone únicamente similitud coseno.
- **Preprocesado y tokenización verificados:**
  - Imagen: reescalado sin recorte (*squash*) a 256×256 con filtro bilineal antialias (`image::imageops::FilterType::Triangle`, equivalente a `PIL.Image.Resampling.BILINEAR` = `resample: 2`), `x / 255.0`, `(x - 0.5) / 0.5`, NCHW RGB.
  - Texto: `tokenizer_config.json` indica `do_lower_case: true`, pero el `normalizer` de `tokenizer.json` solo reemplaza espacios por `▁` y **no pasa a minúsculas** ni trunca (`truncation: null`). La crate pasa a minúsculas antes de tokenizar, trunca a 63 tokens como máximo, añade `<eos>` (`1`) y rellena por la derecha con `<pad>` (`0`) hasta exactamente 64 tokens.
- **`yunet/face_detection_yunet_2023mar.onnx`:** entrada fija `input` **`Float32`** `[1, 3, 640, 640]` en orden BGR sin normalizar (`0..255`), con letterbox arriba-izquierda y mapeo inverso a coordenadas originales; 12 salidas `cls_{8,16,32}`, `obj_{8,16,32}`, `bbox_{8,16,32}`, `kps_{8,16,32}` decodificadas según OpenCV `FaceDetectorYN` (`score = sqrt(cls * obj)`, NMS por IoU).

## DirectML: colocación de nodos y bibliotecas cargadas

Comprobado con `vision_eval nodes` en `dml:0` (registro detallado de ORT):

| Sesión | Nodos en DirectML | Nodos en CPU | Notas |
|---|---:|---:|---|
| SigLIP 2 visión (`fp16`) | 463 | 3 | `Gather`, `Unsqueeze_1`, `Concat_1` en `/vision_model/head/` (decisión de coste de ORT) |
| SigLIP 2 texto (`int8`) | 418 | 62 | Nodos `Gather`/`Unsqueeze`/`Concat` de embeddings y formas de atención |
| YuNet | todos (1 partición fusionada) | 0 | Grafo completo en DirectML |

Bibliotecas cargadas en `target\release\examples\vision_eval.exe`: `C:\WINDOWS\SYSTEM32\DirectML.dll` y `C:\WINDOWS\SYSTEM32\d3d12.dll`; `onnxruntime.dll` no cargada (enlace estático). Todas las sesiones usan ejecución secuencial y desactivan `memory_pattern` en DirectML.

## Verificación sintética (correctitud del pipeline)

Se generaron 20 imágenes sintéticas en código:
- **12 combinaciones forma × color** (`red`, `blue`, `green`, `yellow` × `circle`, `square`, `triangle` sobre fondo blanco).
- **4 colores sólidos** (`solid_red`, `solid_blue`, `solid_green`, `solid_black`).
- **4 maquetas de escena:** página de cómic en escala de grises con viñetas y bocadillos (`scene_comic`), página de documento de texto (`scene_document`), captura simulada de chat móvil (`scene_phone_chat`) y paisaje con degradado de cielo, sol, montañas y lago (`scene_landscape`).

### Recuperación SigLIP 2 (inglés y español)

| Subconjunto | Idioma | Proveedor | Texto → Imagen top-1 (top-3) | Imagen → Texto top-1 (top-3) |
|---|---|---|---:|---:|
| Todos (20) | Inglés | CPU | 17/20 — 85,0 % (20/20) | 20/20 — 100,0 % (20/20) |
| Todos (20) | Inglés | DirectML | 17/20 — 85,0 % (19/20) | 20/20 — 100,0 % (20/20) |
| Todos (20) | Español | CPU | 18/20 — 90,0 % (20/20) | 19/20 — 95,0 % (20/20) |
| Todos (20) | Español | DirectML | 18/20 — 90,0 % (20/20) | 19/20 — 95,0 % (20/20) |
| Forma × color (12) | Inglés y Español | CPU y DirectML | **12/12 — 100,0 %** | **12/12 — 100,0 %** |
| Color sólido (4) | Inglés y Español | CPU y DirectML | **4/4 — 100,0 %** | **4/4 — 100,0 %** |
| Maquetas de escena (4) | Inglés y Español | CPU y DirectML | **4/4 — 100,0 %** | **4/4 — 100,0 %** |

Las filas de cada subconjunto buscan **solo entre sus propias imágenes**; por eso llegan al 100 % aunque la fila «Todos» tenga fallos.

Repetición independiente: la recuperación con DirectML dio las mismas cifras y los mismos fallos; con lote 8, tres procesos por dispositivo dieron 184–188 ms por imagen en CPU y 96–97 ms con DirectML, y la concordancia entre ambos coincidió.

**Fallos registrados (en el conjunto combinado de 20, donde compiten cuadrados grandes de color contra imágenes sólidas del mismo color):**
- **Inglés T→I (CPU):** `solid_red` → `red_square` (0,1519 frente a 0,1511, puesto 2); `solid_blue` → `blue_square` (0,1493 frente a 0,1443, puesto 2); `solid_green` → `green_square` (0,1527 frente a 0,1432, puesto 3; puesto 4 en DirectML con 0,1502 frente a 0,1413).
- **Español T→I (CPU / DirectML):** `solid_blue` → `blue_square` (0,1277 frente a 0,1246, puesto 2); `solid_green` → `green_square` (0,1242 frente a 0,1219, puesto 2).
- **Español I→T (CPU / DirectML):** `solid_green` → `green_square` (0,1236 frente a 0,1219, puesto 2).

### YuNet en imágenes sintéticas

En las 20 imágenes sintéticas (ninguna contiene rostros reales), tanto en CPU como en DirectML:
- Umbral `0,6`: **0 detecciones en 0/20 imágenes**.
- Umbral `0,3`: **0 detecciones en 0/20 imágenes**.

La decodificación de strides 8/16/32 (`score = sqrt(cls * obj)`), el mapeo inverso del letterbox y la supresión NMS se verifican además con tensores construidos en prueba unitaria (`yunet::tests::decode_computes_sqrt_cls_obj_maps_letterbox_and_applies_nms`).

> **Alcance:** estos resultados prueban únicamente que el preprocesado, la tokenización, la inferencia ONNX y el postprocesado funcionan correctamente. **No demuestran precisión alguna sobre fotografías reales, ilustraciones, páginas de manga ni rostros reales.**

## Rendimiento en `--release` (CPU frente a DirectML)

Medido con `target\release\examples\vision_eval.exe matrix --models D:\photosort\models --reps 3 --passes 2`: **3 procesos separados por configuración** en orden alterno (adelante, inverso y rotado a la mitad), separando decodificación PNG y preprocesado de la inferencia pura. Cada celda muestra **mediana [mínimo–máximo]** de los 3 procesos.

- **Visión:** 32 imágenes sintéticas a resolución de página/foto (960×640, 720×960, 450×900) × 2 pasadas = 64 inferencias por proceso tras el calentamiento (`1ª inf`).
- **Texto:** 32 prompts (16 EN + 16 ES) × 2 pasadas = 64 inferencias por proceso.
- **YuNet:** 16 imágenes sintéticas × 2 pasadas = 32 inferencias por proceso (lote fijo 1). En CPU y DirectML la fase estable de YuNet duró ~85–120 ms (< 200 ms del intervalo de muestreo de `nvidia-smi`), por lo que la utilización % de GPU durante YuNet no tiene muestras fiables y se omite.

### SigLIP 2 visión (`vision_model_fp16.onnx`)

| Lote | Proveedor | Hilos | Carga ms | 1ª inf ms | Decod ms/img | Preproc ms/img | Infer ms/img | img/s | RSS pico MiB | GPU antes/pico MiB | GPU % med/máx |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | cpu | 1 | 1002 [992–1029] | 448,3 [437,8–456,4] | 1,50 | 3,50 | 465,78 [453,30–501,80] | 2,1 [2,0–2,2] | 715 | 1261 / 1266 | 0 / 0 |
| 1 | cpu | 4 | 945 [921–977] | 144,3 [141,9–183,0] | 1,47 | 3,67 | 232,78 [230,96–236,12] | 4,5 [4,4–4,5] | 715 | 1261 / 1261 | 0 / 0 |
| 1 | cpu | 6 | 960 [938–966] | 129,0 [117,7–166,2] | 1,53 | 3,47 | 179,94 [178,46–183,07] | 5,7 [5,5–5,8] | 715 | 1261 / 1261 | 0 / 0 |
| 1 | cpu | default | 941 [936–979] | 135,3 [130,5–160,0] | 1,47 | 3,57 | 178,94 [178,82–197,46] | 5,8 [5,2–5,8] | 715 | 1265 / 1265 | 0 / 0 |
| 1 | **dml:0** | default | 833 [825–853] | 314,8 [310,9–316,0] | 1,47 | 3,47 | **103,68 [103,68–103,76]** | **9,6 [9,6–9,6]** | 422 | 1262 / 1495 | 84 / 95 |
| 8 | cpu | 1 | 987 [964–1029] | 4122,0 [4034,5–4358,7] | 1,48 | 3,82 | 509,75 [501,46–547,31] | 2,0 [1,8–2,0] | 926 | 1261 / 1265 | 0 / 0 |
| 8 | cpu | 4 | 923 [916–962] | 1599,0 [1535,1–1664,4] | 1,48 | 3,58 | 253,17 [250,70–256,05] | 3,9 [3,9–4,0] | 927 | 1262 / 1266 | 0 / 0 |
| 8 | cpu | 6 | 956 [930–1019] | 1478,6 [1426,9–1646,3] | 1,50 | 3,66 | 201,78 [200,23–213,60] | 4,9 [4,7–5,0] | 927 | 1261 / 1262 | 0 / 0 |
| 8 | cpu | default | 953 [919–960] | 1403,4 [1374,0–1548,6] | 1,47 | 3,65 | 200,69 [200,61–201,64] | 5,0 [5,0–5,0] | 927 | 1261 / 1261 | 0 / 0 |
| 8 | **dml:0** | default | 840 [833–841] | 999,1 [991,4–1028,5] | 1,48 | 3,71 | **95,42 [95,42–95,47]** | **10,5 [10,5–10,5]** | 435 | 1261 / 1596 | 94 / 99 |
| 16 | cpu | 1 | 959 [950–976] | 8613,4 [8608,2–8650,7] | 1,52 | 3,66 | 547,85 [541,73–548,20] | 1,8 [1,8–1,8] | 1318 | 1261 / 1266 | 0 / 0 |
| 16 | cpu | 4 | 934 [932–940] | 3851,3 [3808,2–3887,2] | 1,48 | 3,59 | 264,09 [261,13–264,66] | 3,8 [3,8–3,8] | 1318 | 1262 / 1265 | 0 / 0 |
| 16 | cpu | 6 | 938 [920–944] | 3223,3 [3168,7–3228,6] | 1,48 | 3,54 | 211,57 [210,19–212,30] | 4,7 [4,7–4,8] | 1319 | 1261 / 1266 | 0 / 0 |
| 16 | cpu | default | 933 [926–947] | 3245,3 [3197,9–3298,4] | 1,50 | 3,77 | 210,40 [210,29–211,31] | 4,7 [4,7–4,8] | 1318 | 1261 / 1261 | 0 / 0 |
| 16 | **dml:0** | default | 845 [826–856] | 1772,5 [1735,6–1776,2] | 1,50 | 3,57 | **95,03 [94,86–95,24]** | **10,5 [10,5–10,5]** | 448 | 1261 / 1718 | 99 / 100 |

### SigLIP 2 texto (`text_model_int8.onnx`)

| Lote | Proveedor | Hilos | Carga ms | 1ª inf ms | Tokeniz ms/u | Infer ms/u | prompts/s | RSS pico MiB | GPU antes/pico MiB | GPU % med/máx |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | cpu | 1 | 1765 [1739–1818] | 55,3 [53,8–56,6] | 0,02 | 55,60 [53,93–59,78] | 17,8 [16,7–18,0] | 565 | 1261 / 1261 | 0 / 0 |
| 1 | cpu | 4 | 1782 [1744–1805] | 24,5 [20,6–24,9] | 0,02 | 22,28 [20,29–24,67] | 38,7 [38,6–41,1] | 565 | 1260 / 1261 | 0 / 0 |
| 1 | cpu | 6 | 1765 [1746–1813] | 24,7 [24,1–25,2] | 0,02 | 18,18 [18,12–25,67] | 48,9 [39,5–49,7] | 565 | 1262 / 1262 | 0 / 0 |
| 1 | **cpu** | **default** | 1744 [1729–1794] | **18,4 [17,5–21,2]** | 0,02 | **17,42 [16,49–28,19]** | **50,9 [35,8–54,0]** | 565 | 1262 / 1262 | 0 / 0 |
| 1 | dml:0 | default | 2116 [2079–2503] | 286,9 [282,0–335,3] | 0,02 | 28,84 [28,82–32,64] | 33,9 [30,8–33,9] | 620 | 1262 / 1602 | 42 / 72 |
| 16 | cpu | 1 | 1771 [1768–1812] | 982,2 [974,1–995,9] | 0,02 | 62,37 [61,10–68,69] | 16,1 [14,6–16,3] | 565 | 1262 / 1262 | 0 / 0 |
| 16 | cpu | 4 | 1758 [1744–1872] | 374,4 [368,1–458,2] | 0,02 | 31,64 [30,49–32,45] | 33,5 [31,7–34,2] | 565 | 1261 / 1262 | 0 / 0 |
| 16 | cpu | 6 | 1737 [1728–1771] | 355,8 [333,0–363,1] | 0,02 | 24,40 [23,83–25,06] | 41,4 [41,2–42,0] | 565 | 1261 / 1261 | 0 / 0 |
| 16 | cpu | default | 1739 [1720–1821] | 350,0 [345,1–394,8] | 0,02 | 24,90 [23,79–25,45] | 40,9 [39,8–42,7] | 565 | 1261 / 1261 | 0 / 0 |
| 16 | **dml:0** | **default** | 2082 [2078–2096] | 592,0 [583,6–612,6] | 0,02 | **17,92 [17,90–17,95]** | **55,7 [55,5–55,8]** | 620 | 1261 / 1977 | 42 / 62 |

### YuNet (`face_detection_yunet_2023mar.onnx`, lote fijo 1)

| Proveedor | Hilos | Carga ms | 1ª inf ms | Decod ms/img | Preproc ms/img | Infer ms/img | img/s | RSS pico MiB | GPU antes/pico MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| cpu | 1 | 56 [55–106] | 11,5 [11,0–11,7] | 1,52 | 7,58 | 8,91 [8,89–9,70] | 108,9 [99,3–110,0] | 194 | 1261 / 1261 |
| cpu | 4 | 56 [56–61] | 5,7 [5,6–5,9] | 1,54 | 7,45 | 3,88 [3,69–4,04] | 251,4 [249,4–262,7] | 195 | 1261 / 1261 |
| cpu | 6 | 59 [59–60] | 5,8 [5,0–6,1] | 1,54 | 7,72 | 4,30 [3,32–4,70] | 227,5 [209,1–285,3] | 195 | 1261 / 1261 |
| cpu | default | 55 [55–56] | 5,5 [5,5–6,6] | 1,55 | 7,48 | 3,44 [3,39–3,76] | 276,2 [256,7–277,5] | 195 | 1261 / 1261 |
| **dml:0** | **default** | 537 [525–548] | 8,8 [8,7–9,5] | 1,56 | 7,77 | **2,66 [2,65–2,67]** | **372,2 [371,3–373,6]** | 362 | 1261 / 1286 |

## Sin medir

- **Precisión sobre fotos reales, ilustraciones, páginas de manga o rostros reales** (ni tasa de acierto de etiquetas zero-shot, ni falsos positivos/negativos de YuNet en caras reales, ilustraciones o anime).
- **Uso simultáneo de VRAM** manteniendo cargados a la vez OCR + SigLIP 2 visión + SigLIP 2 texto + YuNet en los 4 GB de la GTX 1650 SUPER (cada medición cargó una sola sesión por proceso). Al integrar en la app conviene cargar/descargar por etapa o verificar el pico conjunto.
- **Imágenes JPEG/WebP/HEIC reales** desde disco (el benchmark decodificó PNG sintéticos en memoria).
- **Calibración de umbrales de similitud coseno** para asignar etiquetas sugeridas: requiere una muestra real aprobada por el usuario.
- **Estabilidad térmica o de memoria en lotes largos** (miles de imágenes seguidas).

## Cómo reproducirlo

```powershell
$env:PHOTOSORT_VISION_MODELS = "D:\photosort\models"
$env:PHOTOSORT_OCR_MODELS = "D:\photosort\models\ocr"

cargo test -p vision -j 2
cargo run --release -p vision --example vision_eval -- inspect --models $env:PHOTOSORT_VISION_MODELS
cargo run --release -p vision --example vision_eval -- nodes --models $env:PHOTOSORT_VISION_MODELS
cargo run --release -p vision --example vision_eval -- retrieval --models $env:PHOTOSORT_VISION_MODELS --provider cpu
cargo run --release -p vision --example vision_eval -- retrieval --models $env:PHOTOSORT_VISION_MODELS --provider dml:0
cargo run --release -p vision --example vision_eval -- agree --models $env:PHOTOSORT_VISION_MODELS
cargo run --release -p vision --example vision_eval -- matrix --models $env:PHOTOSORT_VISION_MODELS --reps 3 --passes 2
```
