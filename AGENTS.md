# photosort: instrucciones para agentes

Antes de cualquier tarea en este repositorio, lee y sigue `.claude/skills/media-library-catalog/SKILL.md`. Es la guía técnica permanente del proyecto y prevalece sobre cualquier otra instrucción o skill. Lee también sus `references/` cuando la tarea toque exportación, base de datos o uso de IA y costos.

Resumen innegociable:
- Los originales de la biblioteca son de solo lectura. Nunca se borran, mueven, renombran, sobrescriben ni se vuelven a codificar, y no se crean archivos junto a ellos.
- El catálogo (las etiquetas) vive solo en la base de datos de la app. Exportar es siempre una decisión del usuario: se hace con copias byte a byte verificadas con SHA-256 y acompañadas de un manifiesto.
- Ninguna API de pago y ningún envío de datos fuera del PC sin autorización explícita y una estimación de costo previa.
- Nunca se procesa la biblioteca completa para probar algo. Se usan fixtures o una muestra de 100–200 imágenes aprobada por el usuario.
- Commits y PRs según `CONTRIBUTING.md`: sin coautores ni menciones de herramientas de IA.
