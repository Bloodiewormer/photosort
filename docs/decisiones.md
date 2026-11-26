# Registro de decisiones

Formato: fecha · decisión · alternativas · porqué.

- **2026-11-26 · Aplicación de escritorio propia con modelos abiertos locales** (y no Immich, digiKam ni PhotoPrism).
  - Ninguna de esas herramientas ofrece etiquetas múltiples sugeridas con confianza y revisión, entidades de obras en páginas de scanlation y exportación verificada con manifiesto.
  - digiKam, además, puede borrar archivos y escribir metadatos en ellos.
- **2026-11-26 · SQLite + FTS5 como catálogo** (y no Postgres ni una base vectorial).
  - Un solo usuario y unas 35.000 filas.
  - Los embeddings caben en memoria (unos 50–100 MB), así que la búsqueda por fuerza bruta basta.
- ~~Interfaz con pywebview + React/TS~~ (sustituida: ver la decisión de Rust más abajo).
  - Las skills de diseño solicitadas (UI/UX Pro Max, Unslop) trabajan sobre stacks web.
  - WebView2 ya viene con Windows 11.
- **2026-11-26 · Procesamiento 100 % local; ninguna API de pago sin autorización explícita.**
  - Las APIs costarían entre 5 y 153 USD por pasada, según los precios publicados.
  - Además enviarían las fotos privadas fuera del PC.
- **2026-11-26 · Skill Creator: se usa la versión oficial ya sincronizada** (y no la del marketplace), porque es más nueva. Instalar la otra crearía un duplicado.
- **2026-11-26 · Plugins pstack, mattpocock-skills, android, chrisbanes y java-dev desactivados en este proyecto.**
  - Solapaban con Superpowers y GSD e inyectaban instrucciones en conflicto.
  - Solo afecta a este proyecto.
- **2026-11-26 · Compatibilidad con Gemini mediante `AGENTS.md`**, que apunta a la skill (y no con una copia en `.agents/skills/`). Así hay una sola fuente de verdad y no se duplica contenido.
- **2026-11-26 · Interfaz web descartada por el usuario** (sustituye la decisión pywebview + React/TS). Quiere una aplicación de escritorio nativa en **Rust o Java**, para aprender y publicarla en su GitHub. **Decidido: Rust + egui**, todo en un solo proceso (`ort` con CUDA, `oar-ocr`, `rusqlite`). Descartadas: Java con un proceso Python para la IA, Java puro (las librerías de OCR de Java están sin mantenimiento desde 2023-2024 y solo van en CPU) y Slint (licencia triple con obligaciones de atribución). Riesgos aceptados: `ort` sigue en release candidate, `oar-ocr` es un proyecto joven y leer metadatos requiere combinar varias crates.
- **2026-11-26 · Las páginas promocionales distinguen `belongs_to` y `mentions`.** El nombre de archivo indica a qué obra pertenece la página, pero el contenido de una página promocional menciona otras obras. Caso de referencia: ver `references/domain.md` de la skill.
- **2026-11-26 · No se instalan gsd-pi ni claude-mem.** gsd-pi es un agente autónomo completo con `postinstall` que descarga Chromium y se solapa con Superpowers. Claude-mem captura imágenes por defecto y, según fuentes externas, no ahorra tokens. Memoria entre sesiones: `AGENTS.md` + `docs/decisiones.md`; `graphify` solo sobre `src/` cuando haya código, **nunca sobre carpetas con fotos** (envía imágenes a modelos de visión).
- **2026-11-26 · Unslop en espera.** Aprobada por el usuario, pero su herramienta audita interfaces web. Se reevalúa cuando exista la interfaz egui.
- **2026-11-26 · El guardián de rutas se impone con el sistema de tipos.** Las funciones de escritura aceptan solo `DestPath`, que únicamente crea `ensure_writable`; además, `clippy.toml` prohíbe las APIs destructivas de `std::fs`.
