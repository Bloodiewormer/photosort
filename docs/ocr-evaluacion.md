# Evaluación de OCR local (2026-11-26)

La crate `crates/ocr` mide el OCR. **No está conectada con la app ni con el catálogo.** Todo se midió con **imágenes sintéticas** generadas por la propia crate, con títulos y grupos inventados. No se usó ninguna foto ni se accedió a la biblioteca. **Estas cifras no predicen la calidad sobre fotos reales.**

Equipo: Windows 11, Ryzen 5 5600G (6 núcleos / 12 hilos), GTX 1650 SUPER de 4 GB (controlador 610.88, unos 1,2 GB ya ocupados por otros programas), 14 GB de RAM, plan de energía «Equilibrado», Rust 1.100.

## Resumen

- **Selección recomendada: S1.** Los dos reconocedores leen cada línea y gana el coreano solo si al menos el 50 % de sus letras son hangul (H = 0,5) y su puntuación no es más de 0,1 inferior a la de v6 (M = 0,1).
- **Proveedor recomendado en este PC: DirectML con lote 6 en el reconocimiento.** Frente a la mejor configuración de CPU fue 1,8 veces más rápido con tamaños variables (368 ms frente a 677 ms por imagen) y 3,8 veces con formas fijas (78 ms frente a 298 ms). **Con lote 1 y tamaños variables, DirectML no superó a la CPU.**
- Ambos proveedores leyeron **el mismo texto en todas las líneas comparadas**.

## Versiones, archivos y licencias

### Crates

| Crate | Versión | SHA-256 en `Cargo.lock` | Licencia |
|---|---|---|---|
| `oar-ocr` | =0.11.0 | `bd048ac32b74560ee29ad643c7b7b4a62bdc8a8fe7fd360c2117edd680863866` | Apache-2.0 |
| `oar-ocr-core` | 0.11.0 | `e0744c4a4795f98a00b7eda9022061bf3876d3a972a9d1a3cfbb5b8f64903a14` | Apache-2.0 |
| `ort` | =2.0.0-rc.15 | `37c87aa7a90b7ff3e0fe36cccd475df8d67f15f07f4d6c370c4b38464232813c` | MIT o Apache-2.0 |
| `ort-sys` | 2.0.0-rc.15 | `8d22b0417f1838db5a3ac0b3f98dfe39f6fb73624aa0a27c6bbb5040b9c7db2b` | MIT o Apache-2.0 |

Features activas de `oar-ocr`: `download-binaries`, `simd` y `directml`. **`auto-download` y `cuda` no están activas** (tampoco TensorRT, OpenVINO, CoreML ni WebGPU). Se comprueba con `cargo tree -p ocr -e features`.

`simd` acelera el código propio de `oar-ocr`: la normalización de píxeles y el `argmax` de la decodificación CTC (con `wide` y `multiversion`, eligiendo el conjunto de instrucciones al ejecutar). No afecta a las convoluciones. Estuvo activa en los dos proveedores. En CPU, las convoluciones las calcula MLAS, dentro de ONNX Runtime, que elige sus núcleos según la CPU. Esta tiene AVX2 y FMA, y no tiene AVX-512 (comprobado al ejecutar). No se instrumentó qué núcleo concreto eligió MLAS.

### Modelos (no se versionan ni los descarga la crate)

Origen: publicaciones de GitHub `GreatV/oar-ocr`. PaddleOCR, de donde proceden, es Apache-2.0. Los adjuntos de una publicación se pueden reemplazar, así que **el motor no carga nada si falta un archivo o cambia su tamaño o SHA-256**.

| Archivo | Bytes | SHA-256 | Publicación |
|---|---|---|---|
| `pp-ocrv6_small_det.onnx` | 9880512 | `d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e` | v0.7.0 (`4e350472b058ef78a640634bc9095fc3836c3e34`) |
| `pp-ocrv6_small_rec.onnx` | 21159378 | `5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634` | v0.7.0 |
| `ppocrv6_dict.txt` (18708 entradas: kana, CJK y latín; sin hangul) | 74947 | `b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d` | v0.7.0 |
| `korean_pp-ocrv5_mobile_rec.onnx` | 13446374 | `2d7ed96308065a86103325d22af07a88c4d06afc009f21602a4882342c0cc054` | v0.3.0 (`7a0ea9cbe5de7d9e4c8415b032f4854fb688245e`) |
| `ppocrv5_korean_dict.txt` (11945 entradas: 11172 hangul y latín; sin kana ni CJK) | 47451 | `a88071c68c01707489baa79ebe0405b7beb5cca229f4fc94cc3ef992328802d7` | v0.3.0 |

### ONNX Runtime y DirectML

El script de compilación de `ort-sys` descarga **un solo archivo**: `https://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/x86_64-pc-windows-msvc+directml.tar.lzma2`, con SHA-256 `f99aaef23f12986abb05924c944d8433521263ad756b462df762a5a982c98405` en su `dist.tsv`.

Según su código (`build/main.rs` y `build/download/verify.rs`):

- El hash se calcula sobre **el flujo comprimido tal como llega** (`.tar.lzma2`), no sobre los archivos extraídos.
- Se comprueba **después** de extraer en una carpeta temporal. Si no coincide, esa carpeta no se usa y no se enlaza.
- **Si la caché ya existe, no se vuelve a verificar.** La caché está en `%LOCALAPPDATA%\ort.pyke.io`. En esta evaluación no existía antes de la primera compilación.

| Archivo | Bytes | Versión | SHA-256 | Uso | Licencia |
|---|---|---|---|---|---|
| `onnxruntime.lib` | 359948106 | sin recurso de versión; en ejecución informa `rel-1.31.0`, commit `402979a` | `62264c006c6071c95d0a2a08e50082015f8da1dfa2de08a8909086c6f76d0a47` | Enlace **estático**; no se carga ningún `onnxruntime.dll` | ONNX Runtime es MIT (el paquete no trae el texto) |
| `DirectML.dll` (incluida) | 18527776 | 1.15.4+241025-1615.1.dml-1.15.fac7597 | `9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1` | Solo se carga si está junto al `.exe` | Propietaria de Microsoft. El paquete no trae el texto: **revisarla antes de distribuir** |
| `C:\Windows\System32\DirectML.dll` (del sistema) | 10239288 | 1.15.6+260914-0959.1.os-germanium.2f9444f | `19fe59703b10eab58595ea82d3653957493b71238b0124ebaa0c217f8d5313e9` | **La que cargó el programa de medición** | Parte de Windows |
| `C:\Windows\System32\d3d12.dll` (del sistema) | 157576 | 10.0.26100.9818 | `dd6099aa17c756045a1266164d9bbe5c71dd875d44e6411b6519fd19ab5546a1` | Direct3D 12 | Parte de Windows |

**Copias en la carpeta `target`:** `copy-dylibs` dejó `DirectML.dll` en `target\debug\`, `target\debug\examples\` y `target\release\`. Son copias, no enlaces, con el mismo hash que la de la caché. No dejó ninguna en `target\release\examples\`, y por eso las mediciones usaron la de System32 (comprobado con los módulos cargados del proceso). Un `.exe` en `target\release\` cargaría la 1.15.4. Se midieron las dos con el mismo ejecutable, tres veces por configuración: no hubo una diferencia mayor que la variación entre ejecuciones.

## DirectML sin caída silenciosa a CPU

`oar-ocr` 0.11.0 solo usa `error_on_failure` en su sondeo privado. En las sesiones reales, un proveedor que no se registra **cae a CPU sin avisar**. Para evitarlo:

1. El motor crea el único entorno de ONNX Runtime y registra en él DirectML con `error_on_failure`. ONNX Runtime aplica esos proveedores a cada sesión.
2. Entrega ese entorno a `oar-ocr` con `set_ort_environment`.
3. La configuración de sesión usa ejecución secuencial y desactiva el patrón de memoria, como exige DirectML.

Prueba (`tests/directml_strict.rs`): con el dispositivo 99, la carga falla en `dml_provider_factory.cc` («no adapter with the specified ordinal»).

Evidencia de uso real:

- **Reparto de nodos** (registro detallado, `ocr_eval nodes`):

  | Sesión | Nodos en DirectML | Nodos en CPU |
  |---|---|---|
  | Detector | 157 | 0 |
  | Reconocedor v6 | 178 | 2 (`Slice`, `Concat`) |
  | Reconocedor coreano | 298 | 2 (`Slice`, `Concat`) |

  ONNX Runtime manda esos cuatro a la CPU a propósito, porque lo considera más rápido que pasarlos a la GPU.
- **GPU** (`nvidia-smi` cada 200 ms): la memoria pasó de unos 1215 MiB a entre 1665 y 2930 MiB, con un uso medio de entre el 27 % y el 65 %. En las ejecuciones de CPU no cambió. WDDM no da cifras por proceso: son las de toda la GPU.

Un entorno por proceso implica que **no se puede cambiar de proveedor sin reiniciar el proceso**.

## Telemetría y red

- El entorno se crea con `ort::init().with_telemetry(false)`. En `ort` 2.0.0-rc.15 (`src/environment.rs`), eso llama a `DisableTelemetryEvents` y propaga el error si falla. Como solo puede haber un entorno por proceso y `oar-ocr` usa el entregado, ninguna sesión usa otro. La documentación de `ort` indica además que los binarios de pyke no traen la telemetría activada. **No se capturó una traza ETW** para confirmarlo desde fuera.
- Se observaron las conexiones del proceso y de sus hijos durante una medición en CPU y otra en DirectML (135 y 110 muestras): **0 conexiones TCP y 0 puertos UDP**. El único hijo es `nvidia-smi`, local. Es un muestreo, no una captura de tráfico.

## Datos de prueba y medidas

Hay dos conjuntos con semillas y textos distintos: **ajuste** (semilla 1001) y **reservado** (semilla 2002). Los umbrales se eligen solo con el de ajuste. Las cabeceras y las insignias son palabras fijas y se repiten en ambos.

Cada conjunto tiene 48 líneas en cada una de estas categorías: latín (inglés, español con tildes y japonés romanizado), japonés, chino, coreano, estilizado (blanco con contorno negro sobre degradado y figuras), pequeño (mayúsculas de 10 a 16 px) y rotado (de 3 a 8°). Las tres últimas mezclan los cuatro idiomas. Además: 24 columnas verticales en japonés, 6 páginas promocionales de 1340×1280 (13 títulos cada una, de una o dos líneas, con insignias y cabeceras) y 20 imágenes sin texto. Las fuentes son de Windows y se comprueba que tengan todos los glifos.

- El texto se normaliza con NFKC y se colapsan los espacios. El CER y las líneas exactas distinguen mayúsculas.
- Las cajas que el detector parte se vuelven a unir (sin espacio en japonés y chino).
- Un título cuenta como encontrado si sus líneas unidas se parecen en un 0,85 o más, sin distinguir mayúsculas. Las insignias, además, ignoran los espacios.

Parámetros del detector (los de `oar-ocr` para PP-OCRv6): umbral 0,2, caja 0,45, `unclip` 1,4, 3000 candidatos y **`limit_side_len` 960** sobre el lado largo (máximo 4000). Reconocimiento: 48 px de alto y de 320 a 3200 px de ancho.

## Selección del reconocedor

- **S0:** solo PP-OCRv6.
- **S1:** los dos en cada línea. Gana el coreano si su proporción de hangul entre las letras es ≥ H y su puntuación es ≥ la de v6 − M.
- **S2 (cascada):** el coreano solo se ejecuta si v6 no devuelve texto, su puntuación es < T o no hay letras. Después aplica la regla de S1.

**Ajuste.** Se probaron H ∈ {0,2; 0,5; 0,8; 1}, M ∈ {−0,1; 0; 0,05; 0,1; 0,2; 0,4; 1} y T ∈ {0,6; 0,7; 0,8; 0,85; 0,9; 0,95; 0,98; 1,01}. En S1 empatan 16 de 28 combinaciones: cualquier H, con M entre 0,05 y 0,4. Se recomienda un valor central (H = 0,5 y M = 0,1), que da exactamente las mismas cifras que el elegido por el programa.

En S2, T decide el resultado (con H y M del empate, en el conjunto de ajuste):

| T | 0,6 | 0,7–0,85 | 0,9–0,95 | 0,98 | 1,01 (siempre) |
|---|---|---|---|---|---|
| CER medio | 1,36 % | 0,74 % | 0,66 % | 0,48 % | 0,48 % |
| Líneas que van al coreano (de 622) | 131 | 136–137 | 138–143 | 183 | 622 |

### Resultados en el conjunto reservado

Cada celda es **CER / líneas exactas**. S1 y S2 (T = 0,98) dieron cifras idénticas.

| Idioma | S0 | S1 y S2 |
|---|---|---|
| Latín | 0,47 % / 94,1 % | 0,47 % / 94,1 % |
| Japonés | 0,07 % / 99,3 % | 0,07 % / 99,3 % |
| Chino | 0,45 % / 95,2 % | 0,45 % / 95,2 % |
| Coreano | 99,94 % / 0 % | 0,89 % / 87,5 % |
| **CER medio de los 4** | **25,23 %** | **0,47 %** |

En latín, ignorar las mayúsculas no cambia el resultado.

| Categoría | S0 | S1 y S2 |
|---|---|---|
| Latín limpio | 0,13 % / 95,8 % | 0,13 % / 95,8 % |
| Japonés limpio | 0 % / 100 % | 0 % / 100 % |
| Chino limpio | 0 % / 100 % | 0 % / 100 % |
| Coreano limpio | 100 % / 0 % | 0,38 % / 93,8 % |
| Estilizado | 26,02 % / 60,4 % | 1,65 % / 72,9 % |
| Pequeño | 25,19 % / 72,9 % | 0,33 % / 93,8 % |
| Rotado | 25,30 % / 72,9 % | 0,22 % / 95,8 % |
| Vertical (japonés) | 0 % / 100 % | 0 % / 100 % |
| Promocional | 10,38 % / 81,4 % | 0,75 % / 95,0 % |

| Páginas promocionales y falsos positivos | S0 | S1 y S2 |
|---|---|---|
| Títulos latinos | 42/42 | 42/42 |
| Títulos japoneses | 18/18 | 18/18 |
| Títulos coreanos | 0/18 | 18/18 |
| **Títulos, total** | **60/78** | **78/78** |
| Insignias | 78/78 | 78/78 |
| Imágenes sin texto con texto inventado | 8/20 | 8/20 (10 lecturas, 11 caracteres) |

En el conjunto de ajuste, S1 dio un CER medio de 0,48 %, con 74/78 títulos (los 4 que faltan son coreanos). Con DirectML, las tablas de exactitud son idénticas.

**Detección** (conjunto reservado): en las categorías de líneas, 100 % de *recall* y de precisión con IoU ≥ 0,5. En las promocionales, 81,0 % de *recall* y 79,0 % de precisión, aunque **el 100 % de las líneas tiene alguna caja encima**: lo que falla es el solape de las cajas holgadas sobre texto pequeño, como las insignias. Con `limit_side_len` 1344 sube a 92,6 % y 89,6 %, pero el CER medio sigue en 0,47 % y los títulos en 78/78, así que se mantiene 960.

### Qué devuelve v6 ante el coreano

| | Ajuste | Reservado |
|---|---|---|
| Cajas coreanas | 121 | 120 |
| v6 no devuelve texto | 107 | 109 |
| v6 devuelve kana o ideogramas | 13 | 11 |
| Mayor puntuación de v6 | 0,98 | 0,87 |
| Cajas con puntuación ≥ 0,9 | 2 (ambas con ideogramas) | 0 |

Ejemplos: «잠 못 드는» → «吴» con **0,98** (coreano: 0,95); «쌍둥이 동생은» → «号» con 0,87. En ninguna caja no coreana ganó el modelo coreano (0 de 482 y 0 de 483).

### Por qué S1

**v6 inventa ideogramas con mucha confianza.** Por eso S2 solo iguala a S1 con T = 0,98, justo en la puntuación más alta que alcanzó un texto inventado: no queda margen. Además, con `limit_side_len` 1344, el mejor T pasa a 0,95, o sea que depende de la configuración. Si v6 superara T con una lectura inventada, el título coreano se perdería sin ningún aviso.

**Costo** (CPU, lote 6): v6 unos 17 ms por línea y el coreano unos 30 ms. S1 hace 620 reconocimientos coreanos en el conjunto reservado (el 100 %), alrededor de **+179 %** sobre el tiempo de v6. S2 hace 182 (29,4 %), alrededor de **+53 %**. En DirectML con lote 6, con el conjunto variable, el coreano cuesta 179 de los 368 ms por imagen: S2 ahorraría una parte de eso. Conviene reconsiderar S2 solo después de medir las puntuaciones de v6 sobre una muestra real aprobada.

## CPU frente a DirectML

Mismo ejecutable (`--release`), mismas 24 imágenes por régimen, mismos parámetros y S1 en todas las líneas. Cada configuración se ejecutó **3 veces en procesos separados y en orden alterno** (adelante, atrás y empezando por la mitad). Antes y después, la GPU tenía unos 1215 MiB ocupados.

- **Fijo:** todas de 960×960 y con 6 líneas cortas. El detector y el reconocedor reciben siempre la misma forma.
- **Variable:** 24 tamaños distintos, de 1 a 14 líneas de anchos muy diferentes. Hay 155 formas de reconocimiento con lote 1 y 38 con lote 6.
- **Primera pasada:** cada imagen por primera vez, sin contar la primera inferencia, que se informa aparte.
- **Estable:** dos pasadas más sobre las mismas 24 imágenes (48 medidas por proceso).
- **Hilos:** «default» es el valor de ONNX Runtime; se comportó como 6 hilos.
- **Carga:** incluye volver a verificar los SHA-256 (unos 30 ms).
- **Primera inferencia:** en DirectML incluye la compilación de *shaders*. No se vació la caché de *shaders* del controlador, así que no es necesariamente un arranque en frío.

Cada celda es la **mediana de los 3 procesos [mínimo–máximo]**.

### Régimen estable (formas ya vistas)

| Régimen | Lote | Proveedor | mediana ms | p95 ms | imágenes/s |
|---|---|---|---|---|---|
| fixed | 1 | CPU, 1 hilo | 591 [581–597] | 608 [602–674] | 1.70 [1.66–1.72] |
| fixed | 1 | CPU, 6 hilos | 321 [320–322] | 330 [329–333] | 3.11 [3.11–3.12] |
| fixed | 1 | CPU, hilos default | 321 [319–322] | 328 [327–335] | 3.11 [3.11–3.17] |
| fixed | 1 | DirectML | 128 [119–130] | 135 [132–138] | 7.80 [7.67–8.25] |
| fixed | 6 | CPU, 1 hilo | 619 [619–640] | 636 [633–656] | 1.63 [1.56–1.63] |
| fixed | 6 | CPU, 6 hilos | 298 [284–301] | 309 [301–311] | 3.35 [3.32–3.48] |
| fixed | 6 | CPU, hilos default | 308 [295–309] | 326 [302–329] | 3.24 [3.23–3.38] |
| fixed | 6 | DirectML | 78 [78–79] | 81 [81–81] | 12.74 [12.71–12.75] |
| variable | 1 | CPU, 1 hilo | 1472 [1452–1477] | 2444 [2363–2460] | 0.76 [0.75–0.78] |
| variable | 1 | CPU, 6 hilos | 679 [678–689] | 1124 [1049–1136] | 1.65 [1.61–1.70] |
| variable | 1 | CPU, hilos default | 677 [673–713] | 1062 [1057–1160] | 1.68 [1.60–1.69] |
| variable | 1 | DirectML | 820 [808–844] | 1274 [1264–1325] | 1.35 [1.33–1.38] |
| variable | 6 | CPU, 1 hilo | 2107 [2022–2110] | 3901 [3777–4001] | 0.48 [0.47–0.49] |
| variable | 6 | CPU, 6 hilos | 1131 [1086–1204] | 2419 [2256–2576] | 0.86 [0.82–0.88] |
| variable | 6 | CPU, hilos default | 1111 [1104–1127] | 2317 [2219–2342] | 0.88 [0.88–0.88] |
| variable | 6 | DirectML | 368 [361–368] | 636 [633–651] | 2.89 [2.89–2.89] |

### Primera pasada (cada imagen por primera vez) y arranque

| Régimen | Lote | Proveedor | carga ms | primera inferencia ms | mediana ms | p95 ms | imágenes/s |
|---|---|---|---|---|---|---|---|
| fixed | 1 | CPU, 1 hilo | 278 [270–285] | 567 [562–575] | 590 [586–593] | 613 [612–616] | 1.71 [1.70–1.71] |
| fixed | 1 | CPU, 6 hilos | 279 [276–284] | 286 [268–292] | 317 [315–324] | 328 [326–328] | 3.21 [3.16–3.32] |
| fixed | 1 | CPU, hilos default | 272 [268–283] | 302 [243–306] | 317 [317–317] | 324 [324–324] | 3.23 [3.23–3.34] |
| fixed | 1 | DirectML | 673 [661–686] | 556 [549–560] | 123 [118–124] | 133 [129–134] | 8.11 [8.07–8.49] |
| fixed | 6 | CPU, 1 hilo | 268 [267–280] | 619 [618–622] | 626 [617–641] | 657 [646–684] | 1.60 [1.57–1.61] |
| fixed | 6 | CPU, 6 hilos | 279 [265–281] | 303 [283–304] | 293 [292–300] | 304 [299–308] | 3.42 [3.35–3.46] |
| fixed | 6 | CPU, hilos default | 273 [269–275] | 290 [233–300] | 293 [275–316] | 305 [292–332] | 3.48 [3.17–3.75] |
| fixed | 6 | DirectML | 674 [670–676] | 520 [507–525] | 75 [75–76] | 87 [84–93] | 12.92 [12.89–13.06] |
| variable | 1 | CPU, 1 hilo | 280 [275–286] | 1797 [1629–1799] | 1445 [1413–1470] | 2390 [2389–2437] | 0.78 [0.78–0.78] |
| variable | 1 | CPU, 6 hilos | 275 [271–275] | 768 [759–768] | 681 [669–691] | 1079 [1050–1151] | 1.68 [1.65–1.71] |
| variable | 1 | CPU, hilos default | 266 [259–277] | 737 [642–755] | 670 [566–675] | 1090 [1083–1386] | 1.71 [1.32–1.72] |
| variable | 1 | DirectML | 674 [666–676] | 1354 [1325–1355] | 715 [713–734] | 1281 [1273–1285] | 1.39 [1.39–1.42] |
| variable | 6 | CPU, 1 hilo | 279 [261–280] | 2854 [2660–2880] | 2048 [1923–2126] | 3840 [3831–3954] | 0.49 [0.48–0.49] |
| variable | 6 | CPU, 6 hilos | 284 [267–293] | 1567 [1550–1685] | 1186 [1146–1215] | 2412 [2296–2429] | 0.84 [0.84–0.87] |
| variable | 6 | CPU, hilos default | 275 [272–469] | 1632 [1433–1866] | 1144 [1086–1179] | 2288 [2188–2322] | 0.89 [0.87–0.89] |
| variable | 6 | DirectML | 696 [690–706] | 898 [883–908] | 371 [369–395] | 642 [637–652] | 2.83 [2.78–2.84] |

### Reparto por imagen (régimen estable, ms) y recursos

| Régimen | Lote | Proveedor | detección | recorte | rec v6 | rec coreano | RSS pico MiB | GPU antes MiB | GPU pico MiB | GPU uso medio / máx % |
|---|---|---|---|---|---|---|---|---|---|---|
| fixed | 1 | CPU, 1 hilo | 310 [308–314] | 0 [0–0] | 145 [144–145] | 132 [129–144] | 446 [445–446] | 1218 [1213–1220] | 1220 [1218–1222] | 0 [0–0] / 0 [0–0] |
| fixed | 1 | CPU, 6 hilos | 164 [164–165] | 1 [0–1] | 70 [70–71] | 86 [86–87] | 448 [448–449] | 1218 [1213–1219] | 1222 [1216–1222] | 0 [0–0] / 0 [0–0] |
| fixed | 1 | CPU, hilos default | 164 [162–164] | 0 [0–0] | 70 [69–71] | 86 [85–87] | 448 [448–449] | 1216 [1214–1218] | 1218 [1218–1219] | 0 [0–0] / 0 [0–0] |
| fixed | 1 | DirectML | 33 [33–34] | 0 [0–0] | 39 [37–41] | 55 [51–55] | 414 [414–425] | 1216 [1213–1218] | 1667 [1664–1669] | 48 [46–49] / 52 [47–53] |
| fixed | 6 | CPU, 1 hilo | 307 [305–315] | 0 [0–0] | 148 [147–151] | 161 [161–173] | 560 [560–560] | 1216 [1213–1217] | 1223 [1218–1223] | 0 [0–0] / 0 [0–0] |
| fixed | 6 | CPU, 6 hilos | 165 [160–168] | 0 [0–0] | 58 [57–59] | 72 [70–78] | 563 [562–563] | 1216 [1212–1217] | 1216 [1212–1220] | 0 [0–0] / 0 [0–0] |
| fixed | 6 | CPU, hilos default | 165 [161–170] | 0 [0–0] | 58 [57–59] | 79 [72–89] | 562 [562–563] | 1220 [1212–1220] | 1222 [1218–1222] | 0 [0–0] / 0 [0–0] |
| fixed | 6 | DirectML | 33 [33–34] | 0 [0–0] | 19 [19–19] | 26 [26–26] | 457 [456–466] | 1217 [1217–1218] | 1848 [1848–1850] | 65 [64–65] / 66 [65–67] |
| variable | 1 | CPU, 1 hilo | 214 [207–216] | 1 [1–1] | 556 [545–566] | 540 [535–552] | 611 [610–611] | 1216 [1215–1217] | 1222 [1218–1222] | 0 [0–0] / 0 [0–0] |
| variable | 1 | CPU, 6 hilos | 112 [110–113] | 1 [1–1] | 222 [221–228] | 275 [255–278] | 613 [613–613] | 1218 [1213–1222] | 1222 [1218–1222] | 0 [0–0] / 0 [0–0] |
| variable | 1 | CPU, hilos default | 111 [110–113] | 1 [1–1] | 221 [220–221] | 262 [259–290] | 613 [613–614] | 1217 [1212–1218] | 1222 [1218–1222] | 0 [0–0] / 0 [0–0] |
| variable | 1 | DirectML | 56 [55–57] | 1 [1–1] | 286 [281–293] | 397 [390–403] | 518 [517–528] | 1216 [1213–1218] | 1901 [1897–1901] | 27 [26–27] / 36 [35–36] |
| variable | 6 | CPU, 1 hilo | 214 [207–219] | 1 [1–1] | 759 [754–787] | 1115 [1058–1117] | 1326 [1326–1326] | 1217 [1213–1218] | 1223 [1218–1223] | 0 [0–0] / 0 [0–0] |
| variable | 6 | CPU, 6 hilos | 117 [111–119] | 1 [1–1] | 347 [338–362] | 698 [687–733] | 1329 [1329–1329] | 1214 [1213–1220] | 1222 [1219–1224] | 0 [0–0] / 0 [0–0] |
| variable | 6 | CPU, hilos default | 111 [110–112] | 1 [1–1] | 338 [337–338] | 687 [686–690] | 1329 [1329–1329] | 1213 [1210–1218] | 1216 [1215–1222] | 0 [0–0] / 0 [0–0] |
| variable | 6 | DirectML | 47 [46–47] | 1 [1–1] | 119 [118–120] | 179 [179–180] | 895 [893–895] | 1215 [1211–1218] | 2926 [2921–2930] | 47 [47–47] / 55 [54–56] |

### Trabajo realizado y exactitud

| Régimen | Lote | Proveedor | imágenes (estables) | líneas | CER medio % | líneas exactas | tamaños de imagen | formas de rec | textos que cambian al repetir |
|---|---|---|---|---|---|---|---|---|---|
| fixed | 1 | CPU, 1 hilo | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[1,320]] | 0 |
| fixed | 1 | CPU, 6 hilos | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[1,320]] | 0 |
| fixed | 1 | CPU, hilos default | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[1,320]] | 0 |
| fixed | 1 | DirectML | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[1,320]] | 0 |
| fixed | 6 | CPU, 1 hilo | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[6,320]] | 0 |
| fixed | 6 | CPU, 6 hilos | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[6,320]] | 0 |
| fixed | 6 | CPU, hilos default | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[6,320]] | 0 |
| fixed | 6 | DirectML | 24 (48) | 144 | 1.42 | 137/144 | 1 | 1 [[6,320]] | 0 |
| variable | 1 | CPU, 1 hilo | 24 (48) | 173 | 1.96 | 122/173 | 24 | 155 | 0 |
| variable | 1 | CPU, 6 hilos | 24 (48) | 173 | 1.96 | 122/173 | 24 | 155 | 0 |
| variable | 1 | CPU, hilos default | 24 (48) | 173 | 1.96 | 122/173 | 24 | 155 | 0 |
| variable | 1 | DirectML | 24 (48) | 173 | 1.96 | 122/173 | 24 | 155 | 0 |
| variable | 6 | CPU, 1 hilo | 24 (48) | 173 | 2.03 | 122/173 | 24 | 38 | 0 |
| variable | 6 | CPU, 6 hilos | 24 (48) | 173 | 2.03 | 122/173 | 24 | 38 | 0 |
| variable | 6 | CPU, hilos default | 24 (48) | 173 | 2.03 | 122/173 | 24 | 38 | 0 |
| variable | 6 | DirectML | 24 (48) | 173 | 2.03 | 122/173 | 24 | 38 | 0 |

### Conclusión

| Caso | Mejor CPU | Mejor DirectML | Ventaja |
|---|---|---|---|
| Formas fijas, estable | 298 ms (6 hilos, lote 6) | 78 ms (lote 6) | DirectML ×3,8 |
| Tamaños variables, estable | 677 ms (lote 1) | 368 ms (lote 6) | DirectML ×1,8 |
| Tamaños variables, lote 1 | 677 ms | 820 ms | **CPU** |

- **Las formas dinámicas penalizan a DirectML.** Repetir una forma ya vista no la abarata (en la primera pasada y en la estable tarda parecido). Con lote 1 y 155 formas, el reconocimiento tarda más que en CPU (286 + 397 ms frente a 221 + 262 ms), aunque el detector siga siendo el doble de rápido. En una segunda sesión, el mismo caso dio entre 685 y 749 ms: la ventaja de la CPU no es estable, pero DirectML nunca la superó con claridad.
- **El lote cambia el resultado en sentido contrario.** En CPU, con tamaños variables, el lote 6 es peor (1111 ms) porque rellena las líneas cortas. En DirectML, es mucho mejor.
- **Memoria de la GPU:** con lote 6 y tamaños variables, el pico fue de unos 2930 MiB de 4096, es decir, unos 1,7 GB más que antes. Quedaron unos 1,1 GB libres con los demás programas abiertos. Con más programas o líneas más largas podría faltar memoria; no se probó.
- **Arranque:** DirectML tarda más en cargar (unos 675 ms frente a 275 ms).
- **Mismo trabajo y mismo texto:** las mismas imágenes y líneas (144 y 173), y **el texto idéntico en todas las líneas** para v6 y para el coreano (144/144 y 173/173, con ambos lotes). Ningún texto cambió al repetir. El CER del conjunto variable cambia con el lote (1,96 % con lote 1 y 2,03 % con lote 6), pero no con el proveedor.

La app debería usar DirectML con lote 6, mostrar el error si no se puede registrar y dejar que el usuario elija CPU con lote 1. **No hay caída automática.**

## Fallos conocidos

| Caso | Ejemplo (verdad → leído) |
|---|---|
| **PP-OCRv6 no tiene `¡` en su diccionario**; nunca podrá leerlo | «¡Allá va la cigüeña!» → «Allá va la cigüeña!» |
| Se pierden espacios, sobre todo en mayúsculas con contorno | «AND THE PAPER CROWN» → «ANDTHEPAPERCROWN»; «바람 공주» → «바람공주» |
| Chino simplificado leído como tradicional (estilizado y rotado); NFKC no los iguala | «温» → «溫»; «筝» → «箏» |
| Confusión repetida del modelo coreano | «나눠» → «나뉘» |
| **El texto estilizado es la peor categoría** | 72,9 % de líneas exactas; en coreano, 50 % |
| Texto pequeño | 93,8 % exactas (coreano, 83,3 %) |
| Texto inventado donde no hay | 8 de 20 imágenes: `H` (0,34), `GO` (0,43), `一` (0,39), `1` (0,61) |
| v6 con coreano | vacío o ideogramas, a veces con puntuación alta |

Casi todas las lecturas inventadas son de uno o dos caracteres y puntuación baja. Un umbral mínimo podría filtrarlas, pero **no se ajustó ni se midió** cuántas lecturas buenas perdería.

**Texto vertical:** las 24 columnas se leyeron bien, pero son un caso fácil: negras sobre fondo liso, muy separadas, sin furigana, sin `ー` y sin puntuación vertical. No demuestra que funcione con bocadillos de manga reales.

## Sin medir

- **Fotos y escaneos reales.** Hace falta una muestra de 100–200 imágenes aprobada por el usuario, en solo lectura. Aquí no hay compresión JPEG, desenfoque, perspectiva, tramas, manuscrito, tipografías de cartel, texto sobre ilustraciones ni columnas verticales apretadas.
- Rotaciones de más de 8° o texto al revés.
- La asociación de cada título con su portada y las entidades.
- Consumo y estabilidad en ejecuciones largas, y la GPU compartida con carga.
- Otras GPU, otros controladores y varios procesos a la vez.
- La longitud de las líneas y la densidad de texto de una biblioteca real, que deciden el costo por imagen. Por eso no se extrapola a 35.000 imágenes.
- **Indra OCR no se descargó ni se midió:** descargar sus pesos implica aceptar su licencia, y eso solo puede hacerlo el propietario.
- Tampoco se usaron SigLIP, WD Tagger ni CUDA.

## Cómo reproducirlo

En PowerShell, desde la raíz del repositorio. Los cinco modelos deben estar ya en una carpeta; **ningún comando los descarga**. La primera compilación descarga las crates y el paquete de ONNX Runtime.

```powershell
$m = "<carpeta con los cinco archivos>"

cargo test                                   # sin modelos: sus pruebas se saltan con un aviso
$env:PHOTOSORT_OCR_MODELS = $m
cargo test -p ocr --release -- --nocapture   # incluye las pruebas con modelos
cargo tree -p ocr -e features                # sin auto-download ni cuda

cargo run --release -p ocr --example ocr_eval -- eval --models $m
cargo run --release -p ocr --example ocr_eval -- eval --models $m --limit-side-len 1344
cargo run --release -p ocr --example ocr_eval -- eval --models $m --provider dml
cargo run --release -p ocr --example ocr_eval -- nodes --models $m --provider dml
cargo run --release -p ocr --example ocr_eval -- matrix --models $m --reps 3 --passes 2
cargo run --release -p ocr --example ocr_eval -- samples --out "$env:TEMP\ocr-muestras"   # carpeta nueva
```

Las imágenes dependen de las versiones de las fuentes de Windows: otra versión puede cambiar ligeramente las cifras.
