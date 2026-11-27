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

**Al añadir un control:**

- Si su texto en pantalla no lo nombra (campo de texto, lista desplegable, umbral, símbolo como «✕»), usar `theme::named`.
- Un marco o zona que solo reacciona al ratón usa `Sense::CLICK`, para no ser una parada de Tab sin nombre. La acción necesita entonces otro camino con teclado.
- El borde de foco no se dibuja en cada control: `theme::paint_focus_ring` lo pinta por fuera del que tenga el foco, con el color de acento y 2 px. Así se ve también en los botones con borde propio o sin marco. Como el acento cumple 4,5:1 sobre las cuatro superficies en los dos temas, supera el 3:1 que se pide a un borde de foco.
- egui no desplaza la vista al control enfocado: lo hace `theme::scroll_focus_into_view`.

### Qué se comprobó y cómo

La interfaz publica su árbol de accesibilidad con AccessKit, que en Windows lo expone por UI Automation. Va en las características predeterminadas de `eframe` (`accesskit`), que están activas.

`crates/app/tests/accessibility.rs` ejecuta la aplicación real sin ventana, con datos de demostración, y comprueba:

- en galería, sugerencias, duplicados, obras, personas, etiquetas, errores, configuración, bienvenida, galería con selección (barra de selección e inspector), visor y ventana de exportación: todo control o parada de Tab tiene un nombre con letras o cifras;
- cada miniatura se llama como su archivo, más «revisada» y «con sugerencias» cuando corresponde, e indica si está seleccionada;
- los interruptores de Configuración se llaman como lo que controlan, y las opciones excluyentes dicen cuál está elegida;
- con Tab se llega a la búsqueda, la navegación, las miniaturas, la barra de selección, el inspector y Configuración, y el control enfocado queda a la vista;
- la tecla de menú y Mayús+F10 abren el menú de la foto, con el foco en su primera orden. Sobre una miniatura enfocada, el menú es el de esa foto;
- Escape cierra el menú (y devuelve el foco) y la ventana de exportación, sin quitar la selección;
- el foco empieza dentro de la ventana de exportación;
- el borde de foco se dibuja en los dos temas.

Solo revisado en el código: Escape en las ventanas de etiquetas y de propuestas de obras, en el visor y en las listas desplegables. Mientras hay una ventana o un menú abiertos, los atajos de la galería no actúan. En la exportación, Intro solo vale como acción por defecto si ningún control tiene el foco, para que Intro sobre «Cerrar» no inicie una exportación, y solo cuenta una pulsación nueva: Intro mantenido no la inicia al repetirse.

Límites conocidos:

- Las ventanas (exportar, etiquetas, propuestas de obras) y el visor no retienen el foco: con Tab se puede salir a lo que hay detrás.
- El menú contextual de los filtros de la barra lateral solo se abre con el ratón. Con teclado, pulsar el filtro cambia su modo, y las órdenes sobre etiquetas están en «Acciones», en la vista Etiquetas.
- Los nombres que pone egui, como «Close window» y «Resize panel», están en inglés.
- Las pruebas leen el árbol de AccessKit, no lo que dice un lector de pantalla.

### Comprobación pendiente con Narrador

Solo se puede hacer a mano. Con `photosort --demo`:

1. Activar Narrador (Ctrl+Windows+Intro) y recorrer la ventana con Tab: cada parada debe decir un nombre en español y su tipo (botón, interruptor, campo de edición).
2. Pulsar una flecha en la galería: la primera foto queda seleccionada. Ir con Tab a una miniatura: debe oírse el archivo, su estado y si está seleccionada. Comprobar si al mover la selección con las flechas, sin foco en ningún control, Narrador dice algo (se espera que no).
3. Con una foto seleccionada, pulsar la tecla de menú y Mayús+F10: Narrador debe leer cada orden al moverse con las flechas. Escape cierra el menú.
4. En Configuración, comprobar que cada interruptor dice qué controla y si está activado, y que Tema y Dispositivo dicen cuál está elegido.
5. Abrir «Exportar…»: el foco debe empezar en «Carpeta de destino». Escape cierra la ventana.
6. En Personas y Etiquetas, comprobar que los campos de texto y el umbral se anuncian con su nombre.
7. Repetir el recorrido con Tab en los temas claro y oscuro y comprobar a la vista que el borde de foco se distingue, también sobre el botón ocre.

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
