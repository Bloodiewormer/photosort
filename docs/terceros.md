# Licencias de terceros

El código de photosort es MIT. Lo que se distribuye con él conserva su propia licencia. Esto es un resumen para decidir, no asesoría jurídica: manda siempre el texto de cada licencia.

Revisado el 2026-11-27.

## Qué va dentro del ejecutable o a su lado

| Componente | Cómo llega | Licencia | Qué exige al distribuir |
|---|---|---|---|
| ONNX Runtime 1.31 (`rel-1.31.0`) | Enlazado de forma estática en `photosort.exe` | MIT | Incluir su aviso de copyright y el texto MIT. Incluir también su `ThirdPartyNotices.txt` de esa versión, porque la biblioteca estática lleva componentes de terceros (entre ellos Eigen, con MPL-2.0: basta indicar dónde obtener su código, que no se modifica). |
| `DirectML.dll` 1.15.4 | Copiada junto al `.exe` | Microsoft Software License Terms, DirectML (propietaria) | Se puede copiar y distribuir dentro de aplicaciones que uno desarrolla, solo para usarse en Windows o Xbox. No se puede ofrecer suelta, quitar ni alterar los avisos de Microsoft, ni hacer ingeniería inversa. No es una licencia de código abierto y no obliga a publicar el código de la app. |
| 377 crates de Rust | Compiladas en el `.exe` | Permisivas: MIT, Apache-2.0, BSD, Zlib, ISC, BSL-1.0, Unicode-3.0, Unlicense, CC0 | Incluir sus avisos y textos de licencia. Ninguna es solo GPL; la única con opción GPL (`Apache-2.0 OR GPL-2.0-only`) se usa como Apache-2.0. |
| Fuentes de egui (`epaint_default_fonts`) | Dentro del `.exe` | OFL-1.1 y Ubuntu Font Licence 1.0 | Incluir el texto de ambas licencias. |
| `webpki-root-certs` | Dentro del `.exe` | CDLA-Permissive-2.0 | Incluir el texto de la licencia. |

## Modelos

No están en el repositorio ni dentro del `.exe`: el usuario los guarda en una carpeta propia. Si un instalador los incluyera:

| Modelo | Licencia | Qué exige |
|---|---|---|
| SigLIP 2 (`google/siglip2-base-patch16-256`, conversión ONNX de `onnx-community`) | Apache-2.0 | Texto de la licencia y avisos `NOTICE` si existen |
| YuNet (`opencv/face_detection_yunet`) | MIT | Aviso de copyright y texto MIT |
| SFace (`opencv/face_recognition_sface`) | Apache-2.0 | Texto de la licencia y avisos `NOTICE` si existen |
| PP-OCRv6 y reconocedor coreano PP-OCRv5 (PaddleOCR) | Apache-2.0 | Texto de la licencia y avisos `NOTICE` si existen |

Los detalles de versión y SHA-256 de cada archivo están en [ocr-evaluacion.md](ocr-evaluacion.md) y [vision-evaluacion.md](vision-evaluacion.md).

## Antes de publicar un instalador

1. Generar un archivo `THIRD-PARTY-NOTICES.txt` con el texto de licencia de cada crate, de las fuentes de egui y de `webpki-root-certs`, y el `ThirdPartyNotices.txt` de ONNX Runtime 1.31.
2. Mantener `DirectML.dll` junto al `.exe` como parte de la app, nunca como descarga aparte, y sin quitarle sus avisos.
3. Si el instalador incluye modelos, añadir sus licencias.
4. Volver a revisar esta tabla si cambia la versión de ONNX Runtime, DirectML o de algún modelo.

## Fuentes

- Licencia de DirectML 1.15.4: <https://www.nuget.org/packages/Microsoft.AI.DirectML/1.15.4/License>
- Licencia de ONNX Runtime: <https://github.com/microsoft/onnxruntime/blob/main/LICENSE>
- Licencias de las crates: `cargo metadata` sobre `Cargo.lock` para `x86_64-pc-windows-msvc`, solo dependencias de ejecución.
- Reconocedor coreano PP-OCRv5: <https://huggingface.co/PaddlePaddle/korean_PP-OCRv5_mobile_rec>
