# Modelo de dominio y esquema de base de datos

## Conceptos

- **Asset:** un archivo de la biblioteca, identificado por el SHA-256 de su contenido. Si el mismo contenido aparece en dos rutas, son dos ubicaciones del mismo asset; es un duplicado exacto.
- **Categoría:** un árbol editable por el usuario (por ejemplo, Manga › Scanlation › Promocional). Las categorías iniciales vienen de la configuración y el usuario puede crear, renombrar o fusionar más.
- **Etiqueta:** un atributo plano (`portada`, `créditos`, `monocromo`, `texto-en-japonés`).
- **Entidad:** algo con identidad propia, como una obra, una persona (un clúster de rostro), un grupo de scanlation o un personaje. Tiene un nombre canónico y alias, porque una misma obra aparece con varios nombres y en varios idiomas.
- **Tipo de imagen y contenido son dimensiones separadas.** El tipo es foto, captura, escaneo, ilustración o meme; el contenido es persona, anime, tecnología, etc. Una imagen tiene un tipo principal y cualquier número de contenidos.

## Esquema (SQLite; boceto, el código manda)

```sql
asset(id, sha256 UNIQUE, size, width, height, format, phash, exif_json,
      taken_at, created_at_fs, modified_at_fs)
location(id, asset_id, root_id, rel_path, size, mtime, seen_at,
         missing BOOLEAN)                      -- one asset, many paths
library_root(id, path, added_at)

category(id, parent_id, name, source)          -- source: seed | user
tag(id, name UNIQUE)
entity(id, kind, canonical_name)               -- kind: work | person | group | character
entity_alias(entity_id, alias, lang)

asset_category(asset_id, category_id, source, confidence, status, model_ver, updated_at)
asset_tag     (asset_id, tag_id,      source, confidence, status, model_ver, updated_at)
asset_entity  (asset_id, entity_id,   role, source, confidence, status, model_ver,
               region_json)                    -- bbox of the cover/face where it was found
-- role: belongs_to (from filename/folder/sequence) | mentions (found in the image by OCR/vision)
-- source: ai | ocr | rules | user ; status: suggested | accepted | rejected

ocr_text(asset_id, text, lang, boxes_json, model_ver)   + FTS5 index over text
embedding(asset_id, model, vector BLOB)                  -- reuse without re-decoding
analysis_run(id, stage, model, model_ver, prompt_ver, started_at, finished_at)
job(asset_id, stage, state, attempts, error, updated_at) -- resumable queue
dup_group(id, kind)  dup_member(group_id, asset_id, distance)   -- kind: exact | similar
export(id, dest_dir, created_at, options_json)  export_item(export_id, asset_id, dest_path, sha256, status)
```

Reglas:
- Las filas con `status` `accepted` o `rejected` y `source=user` **no se tocan** al volver a analizar. La IA solo inserta o actualiza las filas `suggested` que ella misma creó.
- La cola de revisión es `status='suggested' AND confidence < umbral` (umbral por categoría y configurable).
- La búsqueda combina filtros AND, OR y NOT sobre categorías, etiquetas y entidades, más el texto OCR (FTS5), el nombre de archivo y los metadatos.

## Scanlations de manga y manhwa

Tipos de página (`tag` o `categoría`): `página-capítulo`, `portada`, `créditos`, `anuncio/reclutamiento`, `índice-proyectos/promocional`.

Pipeline propuesto para las páginas promocionales:
1. CLIP o SigLIP con prompts específicos detecta que es una "lista de proyectos con muchas portadas", y no una página con viñetas. Ayudan señales como varias regiones rectangulares tipo portada y mucho texto corto.
2. Se detectan regiones de portada, por ejemplo con contornos rectangulares de OpenCV verificados con CLIP. Más adelante se puede usar un detector de vocabulario abierto.
3. Un OCR multilenguaje (latín, japonés, coreano, chino) extrae líneas de texto, y cada título se asocia con su portada por proximidad.
4. Se normalizan los títulos y se comparan con las entidades `work` existentes y sus alias mediante coincidencia difusa. Si no hay coincidencia, se crea una entidad `suggested`.
5. Resultado: una fila `asset_entity` **por cada obra**, con su `region_json` y su confianza.

Una página promocional **nunca** se resume en una única obra.

**Caso real de referencia** (aportado por el usuario; la imagen no se guarda en el repo, solo su resultado esperado). Archivo `¡Quiero ser alabado por una Gal Gamer! - Capítulo 23.00 - 7.jpg`:
- `belongs_to`: *¡Quiero ser alabado por una Gal Gamer!*, capítulo 23, página 7 (por el nombre).
- Tipo de página: `índice-proyectos/promocional`, con la cabecera "Proyectos activos de" y "Proyectos terminados".
- Entidad `group`: *Los Otakus Fansub* (logo y texto).
- `mentions`: 14 obras, cada una con su portada y su insignia de tipo. Activos: OL and Ghost Arms, Itoshi no Sakura-san, Sampakugan-chan wa Tsutaetai, Koisuru Yankee Girl, Tonari no Kimi ga Ichiban Kawaii, Please, Don't Livestream It! (insignia MANHUA), Sanpaku Eyes Extras, 14-sai no Koi, A Kind World, Midashitai Giya-san to Midarenai, Onee-chan ga Kita, Uchi no Kaisha no Chiisai Senpai no Hanashi. Terminados (ONE SHOT): The Classmate I Hate!, They Were Light Pink.
- Dificultades que debe cubrir la prueba: el texto está estilizado, en blanco con contorno y sobre un degradado; "Sampakugan-chan" y "Sanpaku Eyes Extras" son obras emparentadas con grafías distintas (alias); las insignias MANGA, MANHUA y ONE SHOT dan el subtipo.

Lo que viene después: agrupar páginas de la misma obra por estilo y embeddings, detectar páginas repetidas con pHash y reconstruir secuencias de capítulo.

## Duplicados

- **Exactos:** mismo SHA-256. Es automático y seguro.
- **Similares:** distancia de Hamming de pHash baja, o similitud de embeddings alta. Cubre la misma imagen en otra resolución, recortada, recomprimida, o páginas repetidas.
- Se muestran agrupados para que el usuario los revise. **No hay ninguna acción de borrado**: como mucho, el usuario marca cuál es la versión preferida, y eso solo queda en el catálogo.
