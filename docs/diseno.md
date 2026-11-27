# Diseño de la interfaz

## Dirección

- **Producto:** un catálogo local de una biblioteca personal de fotos que nunca toca los originales.
- **Carácter:** de archivo, exacto, sereno.
- **Mundo:** el archivo fotográfico: hojas de contacto y las marcas de lápiz graso que se hacen sobre las pruebas.
- **Momentos:** las miniaturas. El color lo ponen las fotos; el resto se queda callado.
- **Firma:** la marca de lápiz graso ocre sobre las fotos revisadas, y en el logo. Un solo detalle, siempre con el mismo significado.

## Reglas

### Organización

- **Navegación a la izquierda:** biblioteca (todas, sin revisar, revisadas), sugerencias, duplicados, obras y etiquetas; la configuración, abajo. Debajo, los filtros, en grupos que se pliegan.
- **Barra superior mínima:** nombre, búsqueda y estado de la revisión con su botón de pausa.
- **Lo contextual aparece cuando hace falta:** la barra de selección solo existe con algo seleccionado, y el inspector, solo con una foto elegida.
- **Configuración** es una pantalla propia, con pocas opciones agrupadas por tema y valores por defecto sensatos. No contiene acciones del trabajo diario. Interruptores para sí/no, opciones excluyentes visibles para elegir una, y «Acerca de» al final.

### Menú contextual

- Solo órdenes que se aplican a lo seleccionado, en tres grupos como máximo.
- **Nunca es el único camino** a una acción importante: lo frecuente también está en la barra de selección o en el inspector.
- Se abre con clic derecho, con la tecla de menú y con Mayús+F10. Clic derecho sobre una foto no seleccionada la selecciona; sobre una seleccionada, conserva la selección.
- La interfaz dice que existe.

### Estilo

- **Superficies neutras** de grafito, apenas cálidas. Sin degradados, brillos ni sombras de color.
- **Un acento**, el ocre del lápiz graso, para la acción principal, la selección, el foco y la marca de revisada. Ni azul ni violeta.
- **Color con significado:** oliva para aceptado y óxido para rechazado, siempre acompañado de texto o forma.
- **Espaciado** en múltiplos de 4. Mismo elemento, mismo tamaño.
- **Tipografía:** una familia, pocos tamaños, mínimo 13 px, jerarquía por tamaño y color.
- **Texto:** frases cortas en minúscula de oración, sin exclamaciones, sin punto final en etiquetas y botones. Sin emojis.
- **Movimiento:** solo el que explica algo. Nada en bucle.

### Accesibilidad

- Contraste de texto de al menos 4,5:1 en los dos temas, comprobado por una prueba.
- Foco visible, todo alcanzable con teclado y controles con nombre.
- Ningún estado se distingue solo por el color.

## Paleta

| Uso | Oscuro | Claro |
|---|---|---|
| Fondo | `#151514` | `#f2f1ec` |
| Panel | `#1c1c1a` | `#faf9f5` |
| Tarjeta | `#242422` | `#ffffff` |
| Elevado | `#2f2f2b` | `#e9e7df` |
| Borde | `#3d3c37` | `#cdc9bb` |
| Texto | `#ecebe6` | `#1d1c19` |
| Texto secundario | `#b6b4aa` | `#4c4a42` |
| Texto de ayuda | `#a09e93` | `#5c594f` |
| Acento como texto o foco | `#d9a416` | `#765500` |
| Fondo de selección | `#3a3018` | `#f4e7bd` |
| Aceptado | `#a3b870` | `#4a5e1e` |
| Rechazado o peligro | `#e8957d` | `#97381f` |
| Exigir (fondo / texto) | `#2d3520` / `#d6e2ae` | `#e3ebc9` / `#34460f` |
| Alguna (fondo / texto) | `#3a3018` / `#f0d38a` | `#f4e7bd` / `#5c4200` |
| Excluir (fondo / texto) | `#3f241d` / `#f2c3b4` | `#f6d9cf` / `#6f2413` |

Relleno del botón principal y de la marca, en ambos temas: `#d9a416` con texto `#1c1608`.

## Logo

`assets/icon.svg`: una hoja de contacto de cuatro pruebas, con una elegida y rodeada de un solo trazo desigual. Dos colores planos sobre grafito. `assets/icon.png` es la misma imagen a 256 px, para la ventana.

## Fuentes

- [Guía de configuración de aplicaciones de Windows](https://learn.microsoft.com/en-us/windows/apps/design/app-settings/guidelines-for-app-settings)
- [Guía de menús de Windows](https://learn.microsoft.com/en-us/windows/win32/uxguide/cmd-menus)
- [NN/g: menús contextuales](https://www.nngroup.com/articles/contextual-menus-guidelines/)
- [NN/g: revelación progresiva](https://www.nngroup.com/articles/progressive-disclosure/)
- [Apple: menús contextuales](https://developer.apple.com/design/human-interface-guidelines/context-menus)
- La revisión contra el aspecto genérico sigue las pasadas de [Unslop](https://github.com/heydamir/unslop), de Damir Sokolovsky (CC BY 4.0). Está pensada para la web; aquí se aplican sus criterios a una aplicación de escritorio.
