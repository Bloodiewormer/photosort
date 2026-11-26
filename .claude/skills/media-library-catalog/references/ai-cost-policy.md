# Política de IA, costos y privacidad

## Dos decisiones distintas

1. **Modelos para desarrollar** (Claude o Gemini, como asistentes de programación). Se consumen de la suscripción o los créditos del usuario. Se usa el más económico que haga bien la tarea:
   - Haiku o Gemini Flash para tareas mecánicas y búsquedas;
   - Sonnet o un Gemini equivalente para programar y revisar;
   - modelos de gama alta solo cuando un problema lo justifique de verdad;
   - **Fable nunca**.
2. **Modelos que usa la aplicación para clasificar.** Por defecto son locales y abiertos, ejecutados en la GPU del usuario (GTX 1650 Super, 4 GB de VRAM), así que procesar 35.000 imágenes cuesta electricidad, no dinero.

## Preferencia económica (en orden)

1. Local y gratuito.
2. Modelo abierto ejecutado en el PC del usuario.
3. Híbrido: local por defecto y un servicio externo solo para un subconjunto pequeño donde aporte una ventaja medible. Siempre opcional y claramente marcado.
4. API de pago solo con una justificación técnica y económica, y con autorización.

## Protocolo antes de cualquier operación con costo o envío externo

No ejecutes la llamada. Primero presenta al usuario:
- el servicio, el modelo y el **precio publicado vigente**, con URL y fecha de consulta;
- el costo por imagen, por 1.000 y por 35.000 imágenes, incluidos OCR, embeddings y reintentos (añade un 10–20 % de margen);
- qué datos salen del PC (imágenes, texto, metadatos) y la política de retención del proveedor;
- la alternativa local equivalente y la diferencia esperada de precisión y velocidad;
- una propuesta de piloto de 100–200 imágenes para **medir** el consumo real antes de extrapolar.

Espera un "sí" explícito para esa operación concreta. Un permiso anterior no se extiende a otra operación. Distingue siempre entre "precio publicado" y "costo medido".

## Privacidad

- No se envían a terceros, sin permiso expreso, imágenes, miniaturas, texto OCR, nombres de archivo, rutas, EXIF, GPS ni vectores de rostros.
- Las claves API viven solo en variables de entorno o en un `.env` ignorado por git. Nunca van en el código ni en logs.
- Nada de telemetría, ni propia ni de librerías. Desactívala si la librería la trae, por ejemplo con `HF_HUB_DISABLE_TELEMETRY=1` y `DO_NOT_TRACK=1`.

## Pruebas y calidad

- La lógica se prueba con fixtures sintéticos. La calidad real se mide con una muestra de 100–200 imágenes **elegida con el usuario**, solo en lectura.
- Nunca se lanza la biblioteca completa para probar o depurar una función.
- Los modelos se comparan con métricas medidas en la misma muestra: precisión por categoría, imágenes por segundo y VRAM. No basta con la reputación del modelo.
