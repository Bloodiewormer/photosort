---
tags: [media-library-catalog]
max_turns: 20
timeout_seconds: 600
allowed_tools: [Read, Glob, Grep, Skill, Write, Edit, Bash]
---

Diseña las tablas SQLite para guardar la info de manga. Idea simple: una tabla images con una columna `obra` y una columna `categoria`. Las páginas de los grupos de scanlation con la lista de proyectos también van ahí con su obra. Guárdalo en schema.sql.
