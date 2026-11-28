# Reglas de contribución

Este proyecto trabaja junto a una biblioteca personal de fotos irreemplazable. Por eso todo cambio se trata con el máximo cuidado.

## Commits

1. **Sin coautores ni menciones de herramientas de IA.** Ningún commit lleva líneas `Co-Authored-By`, pies tipo "Generated with…" ni nombres de asistentes, modelos o plataformas de IA (Claude, Anthropic, T3, ChatGPT, OpenAI, Gemini, Copilot, etc.). El autor es únicamente la persona configurada en `git config user.name`.
2. Un commit hace **un solo cambio lógico**, con un mensaje claro en imperativo que explique *qué* cambia y *por qué*.
3. Antes de cada commit:
   - `cargo fmt --check`, `cargo clippy -- -D warnings` y `cargo test` pasan;
   - el diff se ha revisado línea a línea (`git diff --staged`);
   - no se incluyen datos personales, rutas de la biblioteca, fotos, catálogos (`*.sqlite`), registros ni secretos (`.env`).
4. Prohibido `--no-verify`, `--amend` sobre commits ya publicados, y `push --force` a `master`.

## Pull requests

1. Nunca se trabaja directamente en `master`: cada cambio va en una rama y llega por PR.
2. La descripción del PR explica qué cambia, por qué, y cómo se ha verificado. Debe decir explícitamente si el cambio **afecta a la seguridad de los originales**.
3. Las mismas prohibiciones del punto 1 de los commits se aplican al título y al cuerpo del PR.
4. Si un PR toca la lectura de originales, la copia o la exportación:
   - debe incluir tests de integridad (comprobación de hash antes y después);
   - debe revisarse antes de fusionarlo.
5. No se fusiona nada con tests en rojo.

## Seguridad de los originales (regla innegociable)

- El código no puede borrar ni modificar archivos de la biblioteca de origen. Solo los mueve, por acción expresa del usuario, a través de `fsguard::move_no_replace` y `fsguard::recycle`. `tests/safety.rs` y las reglas de `clippy.toml` lo comprueban.
- Los originales solo se abren en modo lectura (`File::open`), y toda escritura pasa por el guardián de rutas.

## Flujo de trabajo

1. Cada cambio parte de una **issue**.
2. Rama desde `master` actualizado: `feat/…`, `fix/…` o `docs/…`. No se apilan ramas sobre otras sin fusionar.
3. PR hacia `master`. En la descripción, `Closes #N` por cada issue resuelta, para que se cierre al fusionar.
4. GitHub ejecuta en cada PR:
   - el control de mensajes, con la misma regla que el hook local, sobre commits, título y descripción;
   - formato, análisis estático y pruebas en Windows.
   Las pruebas que necesitan modelos aparecen allí como omitidas: **hay que ejecutarlas en local** cuando se toque el OCR o el reconocimiento.
5. Se fusiona cuando todo está en verde.

## Pruebas con modelos

Están marcadas con `#[ignore]`. Si las pides sin definir las carpetas, fallan: nunca se aprueban sin hacer nada.

```powershell
$env:PHOTOSORT_OCR_MODELS = "<carpeta de modelos OCR>"
$env:PHOTOSORT_VISION_MODELS = "<carpeta que contiene siglip2 y yunet>"
cargo test -p ocr -p vision -- --include-ignored
cargo test -p app --test ocr_e2e --test works_e2e --test vision_e2e --test review_e2e -- --include-ignored
```

Las mediciones de rendimiento también están marcadas con `#[ignore]` y se ejecutan una a una con `--release`.

## Activar los hooks (una vez por clon)

```sh
git config core.hooksPath .githooks
```
