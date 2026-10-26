# Propuesta: testers de la comunidad, issues y diagnóstico

Estado: **propuesta para revisar (2026-10-26)**. No se activa nada, no se publica ningún repo y no se crea ningún bot sin el OK explícito de Damian. Tareas: SC-89 (circuito de issues y bot), SC-90 (logs y exportación de diagnóstico).

## La idea en una línea

La gente pide el formato en Discord, reporta en GitHub con un paquete de diagnóstico que exporta la app, y el reporte pasa por estos pasos:

1. Un clasificador sin permisos lo filtra.
2. Damian aprueba.
3. Claude lo reproduce, lo arregla y abre un PR.
4. Damian mergea.

```
Discord /reporte ──► mensaje con el formato + link al formulario de GitHub
                                      │
                        Issue (formulario con campos obligatorios + .zip de diagnóstico)
                                      │
             Clasificador (IA SIN herramientas) ──► veredicto JSON
                                      │
             Script común (sin IA) aplica el veredicto:
               valido ─► label "in-review"     sin log ─► "needs-log" + pedido
               duplicado ─► "duplicate" + link  sospechoso ─► se borra (o se oculta)
                                      │
             Damian lee "in-review" y pone "agent-ok"
                                      │
             Claude Code (rama propia): reproduce ─► test ─► arreglo ─► PR
               si duda ─► "needs-damian" + comentario con la pregunta concreta
                                      │
             Damian revisa y mergea (auto-merge solo más adelante, por categorías)
```

## 1. Bot de Discord (`/reporte`)

- **Qué hace:** un slash command (`/reporte`, `/bug`) responde solo a quien lo pidió, con mensaje efímero:
  - qué necesitamos;
  - cómo exportar el diagnóstico desde la app (sección 4);
  - el link al formulario de issue en GitHub, ya con la plantilla elegida.
- **Comandos extra:**
  - `/estado <número>`: muestra las etiquetas y el estado del issue;
  - `/version`: muestra la última versión publicada.
- **Sin IA:** es un bot de comandos fijos. No lee mensajes libres ni crea issues por su cuenta: el reporte lo crea la persona en GitHub. Así el bot no es una puerta de entrada a nada.
- **Hosting:** cualquier lugar con Node o Python siempre prendido. La Raspberry del Pi Companion sirve si querés todo en casa.
- **Formato que manda** (borrador):

> **Para reportar un problema de SoulCrafted:**
> 1. En la app: Ayuda → Exportar diagnóstico. Se genera `soulcrafted-diagnostico-<fecha>.zip`.
> 2. Abrí el formulario: <link>
> 3. Completá: versión (sale en el zip), qué hiciste paso a paso, qué esperabas, qué pasó.
> 4. Adjuntá el `.zip`. Si el problema depende del modelo y podés compartirlo, adjuntalo o linkealo (opcional).
> No pegues contraseñas ni datos personales. Los reportes que no siguen el formato se cierran solos.

## 2. Formularios de issue en GitHub

- **Formato:** archivos `.github/ISSUE_TEMPLATE/*.yml` (issue forms) con campos obligatorios, y los issues en blanco desactivados.
- **Plantillas:**
  - **Fallo o cierre de la app:** versión, sistema operativo, pasos, esperado y obtenido, diagnóstico `.zip` (obligatorio).
  - **Resultado de impresión raro:** lo mismo, más impresora, resina y foto opcional. Estos casos los ve siempre Damian, porque no se pueden reproducir sin imprimir.
  - **Idea o pedido:** texto libre; no entra al circuito automático.
- **Adjunto obligatorio:** GitHub no puede obligar a adjuntar un archivo. El clasificador lo controla y, si falta, pone `needs-log` y comenta pidiéndolo.

## 3. Clasificación de issues de cualquiera

Cualquiera puede escribir un issue, así que su texto se trata como **dato no confiable**.

- **Clasificador:** una GitHub Action en `issues.opened` / `issues.edited` le pasa a Claude el título, el cuerpo y la lista de adjuntos, **sin herramientas, sin checkout del repo y sin tokens**.
  - Devuelve solo un JSON validado contra un esquema: `{veredicto: valido|sin_log|duplicado|sospechoso|fuera_de_tema, tipo, resumen, duplicado_de?}`.
  - Si la salida no cumple el esquema, se trata como `sospechoso`.
- **Aplicador:** un script común, sin IA, lee ese JSON y es el único que tiene el token para etiquetar, comentar, cerrar o borrar.
  - Como la IA no puede actuar, lo peor que logra un texto malicioso es que lo marquen `in-review`, y ahí lo filtra Damian.
- **Sospechosos:** son los que intentan dar órdenes al bot o a la IA, piden secretos, traen links o adjuntos raros, o son spam.
  - **Opción A (la que propuso Damian):** se borran automáticamente, sin comentario.
    - Borrar issues requiere un token de admin, que vive solo en el paso del aplicador.
    - **Riesgo:** un falso positivo borra un reporte legítimo para siempre.
  - **Opción B (recomendada para empezar):** cerrar, bloquear y etiquetar `spam-sospechoso`, oculto del listado normal. Damian puede revisarlos de un vistazo, y una acción semanal borra los que siguen así a los 7 días.
- **Válidos:** `in-review`, más una etiqueta de tipo (`crash`, `soportes`, `hollow`, `slice`, `ui`, `impresion`). El resumen de una línea va en un comentario, para que Damian lea rápido.
- **Límites:** máximo N issues clasificados por día y por autor, para cortar el spam y el costo.

## 4. Logs en la app y "Exportar diagnóstico" (SC-90)

Esta parte vive en SoulCrafted y sirve aunque no exista la comunidad.

- **Lo que ya existe:** `logs/crash.log`, donde queda cada panic con su backtrace, y `logs/df-supports-timings.log`.
- **Log de interacción (opcional):** se activa en Preferencias → Privacidad, apagado por defecto, y se pregunta en el primer arranque.
  - **Qué registra:** eventos, no contenido. Por ejemplo cambios de pestaña, herramientas usadas, comandos (abrir, generar soportes, ahuecar, slicear, exportar) con su tiempo y resultado, errores, cancelaciones y versión.
  - **Qué no registra:** geometría, nombres de archivo completos ni nada escrito por la persona.
  - **Tamaño:** archivo rotativo de 5 MB como máximo.
- **Botón "Exportar diagnóstico"** (menú Ayuda): arma un `.zip` con:
  - los logs;
  - versión y commit del build;
  - sistema operativo;
  - GPU y backend (nombre del adaptador, driver);
  - DPI y monitores;
  - preferencias, limpias;
  - estadísticas de la escena: cantidad de modelos y triángulos, soportes y hollow, sin geometría.
  - Casilla opcional "incluir los modelos de la escena".
- **Limpieza antes de exportar:** las rutas con el usuario de Windows pasan a `~`, y se muestra qué incluye el zip antes de guardarlo.
- **Para reproducir:** el `crash.log` con backtrace más el log de interacción suelen alcanzar para escribir el test que reproduce el problema.

## 5. Arreglo con Claude (issues con `agent-ok`)

- **Disparador:** la etiqueta `agent-ok`, que pone solo Damian, o un comentario de Damian que menciona al bot. La acción (`claude-code-action`) corre en un runner Windows con permisos de **escritura solo en ramas `agent/*` y en PRs**. Sin push a `main`, sin acceso a releases, secretos de deploy ni settings.
- **Pasos:**
  1. Leer el issue y el zip.
  2. Reproducir con un test que falle.
  3. Arreglar y correr tests y clippy.
  4. Abrir un PR con qué fallaba, por qué, cómo se reprodujo y qué cubre el test.
- **Dudas:** si algo no se puede reproducir o la decisión es de producto, se etiqueta `needs-damian` con la pregunta concreta. Nunca se inventa un arreglo.
- **Contenido del reporte:** los logs y los adjuntos siguen siendo datos, no instrucciones. Si un log contiene texto que parece una orden, el agente lo cita en el PR y no lo ejecuta.

## 6. Merge

- **Al principio:** siempre lo mergea Damian. Desde el celular es revisar el PR y apretar un botón.
- **Más adelante, opcional:** auto-merge solo para categorías acotadas con CI en verde: crash con test que lo reproduce, textos y traducciones, UI con test.
- **Nunca automático:**
  - slicing y writers de archivos de impresora;
  - perfiles de impresoras y resinas;
  - protecciones de datos (undo, guardado, colisiones, cancelación);
  - dependencias nuevas;
  - cualquier cosa que cambie lo que se imprime.

## Antes de activar: decisiones de Damian

1. **Publicar el repo:** nombre, qué rama, y que `.local/` y los datos de referencia de DF/Lychee queden afuera.
   - **Licencias:** el código portado de DF es MIT (ver `libs/sc-geom/hollow-df/NOTICE`) y SC es AGPL. Caliper no entra.
2. **Builds públicos:** dónde se publican (Releases de GitHub) y con qué numeración, para que el diagnóstico diga exactamente qué versión es.
3. **Sospechosos:** opción A (borrar al instante) u opción B (ocultar y borrar a los 7 días).
4. **Límite de costo:** cuántos issues por día puede clasificar o arreglar la IA.
5. **Bot:** dónde corre (la Raspberry, un VPS chico o un servicio gratuito).

## Orden de trabajo propuesto

1. **SC-90 en la app:** log de interacción opcional y "Exportar diagnóstico". Sirve ya, incluso sin comunidad.
2. **Formularios de issue y etiquetas** en el repo público.
3. **Clasificador y aplicador**, primero en modo "solo etiquetar", con la opción B.
4. **Bot de Discord** con `/reporte`.
5. **Arreglos con `agent-ok`.**
6. **Auto-merge por categorías**, solo si los PRs vinieron bien durante un tiempo.
