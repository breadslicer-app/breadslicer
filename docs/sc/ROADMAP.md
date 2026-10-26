# Roadmap propio de SC

Snapshot de planificación 2026-10-22, posterior al inventario SC-09. **83 tarjetas**; el estado operativo está en [Workboard](https://oc.synrgs.tech/workboard/sc).

No es una promesa de implementar todo. Las ideas son exploratorias, y las tarjetas de hitos agrupan trabajo sin duplicar sus entregables. SC-08/09/14 y SC-24 están completadas en sus alcances; las demás se organizan y no se iniciaron por cerrar el corpus. No hay despacho automático.

## Orden actual (2026-10-26, con Damian)

La línea de soportes DF funciona de punta a punta: modelo → soportes y raft → slice → archivo validado en UVTools (SC-79, SC-80, SC-81). La UI nueva está en marcha (SC-84). Ahora toca **terminar el slicer**; la optimización a fondo va después, cuando se sepa qué flujos se usan de verdad. Bloques en orden:

1. **Transform completo** (SC-26, SC-27, SC-85):
   - medidas reales del objeto seleccionado;
   - mover, rotar y escalar con números y viendo cuánto se movió o rotó;
   - lift, apoyar en cara y centrar;
   - ayudas visuales del gizmo (grados al rotar, línea del eje como Blender).
2. **Soportes completos** (SC-86, SC-83):
   - todas las opciones en un panel, más el lift del objeto;
   - presets, empezando por los de Damian ("MIni - 35mm - Tall" y "Figure - 75mm +", SC-07);
   - cambiar un ajuste re-planifica sin re-escanear.
3. **Hollow** (SC-18, SC-39…46): portar el motor de DragonFruit con las mejoras de Damian, más drenajes y áreas sólidas, en la pestaña Hollow.
4. **Soportes manuales** (SC-10, SC-11, SC-12): agregar, borrar, mover punta y brace en el viewport.
5. **Repair** (SC-87): mejorar el repair propio con lo mejor de DragonFruit; Caliper como extra, con un botón que lo abre o enlaza su descarga.
6. **Slice y salida:**
   - estimaciones de tiempo, resina y costo, y chequeos;
   - zoom y paneo de la vista de capa (SC-88, SC-34);
   - writer Anycubic (SC-19, SC-36).
7. **Optimización** (SC-82 en pausa, SC-77): router, escaneo de islas y GPU.
8. **Primera impresión física** (SC-66), cuando Damian la autorice. Conviene no dejarla para el final: valida todo lo anterior.

## Orden inmediato recomendado (2026-10-22, histórico)

1. SC-24: corpus representativo **completado** (25 casos / 28 assets).
2. SC-14: flujo guardar/abrir/historial **completado**, con corrección de protección dirty.
3. SC-10: especificar primer hito de soportes manuales.

Después: SC-37/38 → SC-11 → SC-12. Hollow: SC-39/40 antes del motor. La validación de perfiles/writers puede avanzar como línea independiente.

### Línea de soportes DF (2026-10-25)

El porteo de soportes de DF a Rust nativo terminó (bloques 1–9 en `docs/sc/DF-PORT.md`, rama `sc/df-supports`). Orden acordado con Damian: que funcione de punta a punta, después que sea correcto en casos reales, recién después que sea rápido.

1. SC-78: fachada del core de soportes (una entrada, cancelable, con progreso y errores tipados).
2. SC-79: rebanada vertical en la GUI (modelo → auto-soportes → raft → preview → slice → exportar), sin interacciones finas.
3. SC-80 y SC-81: arreglar el stitching de `sl1cer` y validar soportes y raft en el slice y el archivo final.
4. SC-82: benchmarks del pipeline en escenas reales, cada uno con una pregunta concreta.
5. SC-75 y SC-76: mejoras detrás de opciones, primero las de corrección, con la paridad DF intacta al apagarlas.
6. Rendimiento: paralelismo en CPU donde SC-82 lo pida, después GPU (SC-77, SC-22).
7. Interacciones de edición (SC-12) con pruebas de gestos de Damian; validación física (SC-66) solo con autorización.

## Cómo mantenerlo

- Leer estado actual del board antes de editar; conservar IDs y evidencia histórica.
- Cerrar una tarea sólo con aceptación demostrada y commit/build/artefacto asociado. Inventario estático no significa runtime probado.
- A cada entrega, regenerar INVENTORY.json y ajustar la matriz y este snapshot si cambia el alcance. Guardar snapshot operativo con IDs si se modifica el board.
- Usar dependencias como orden de planificación; no promueven ni lanzan agentes automáticamente.
- En Workboard se muestran como enlaces informativos `blocked_by`, no como dependencias que promueven tarjetas a ejecución.
- No agregar librerías o servicios por una idea: preflight de opciones abiertas, mantenimiento/licencia y medición antes de elegir.

## Índice completo

| ID | Estado snapshot | Área | Tarea | Depende de |
|---|---|---|---|---|
| [SC-01](#sc-01) | done | base | Compilacion nativa y validacion de componentes centrales | — |
| [SC-02](#sc-02) | done | editor | Gizmo combinado de mover, rotar y escalar | — |
| [SC-03](#sc-03) | done | editor | Contacto exacto con la cama al rotar y escalar | — |
| [SC-04](#sc-04) | done | shortcuts | Transformaciones modales G/R/S con X/Y/Z | — |
| [SC-05](#sc-05) | done | camara | Camara Blender, encuadre y perspectiva/ortografica | — |
| [SC-06](#sc-06) | done | camara | Radial con tecla ordinal, swipe y confirmacion al soltar | — |
| [SC-07](#sc-07) | done | investigacion | Revision del informe Lychee y catalogo de presets | — |
| [SC-08](#sc-08) | done | base | Inventario, respaldo privado GitHub e índice mantenible | — |
| [SC-09](#sc-09) | done | arquitectura | Inventario del core y matriz GUI/core/evidencia/brechas | SC-08 |
| [SC-10](#sc-10) | todo | soportes | Especificar primer hito vertical de soportes manuales | SC-09 |
| [SC-11](#sc-11) | backlog | soportes | Modelo paramétrico persistente y geometría de soportes | SC-10, SC-37, SC-38 |
| [SC-12](#sc-12) | in progress | soportes | Editor manual de soportes en viewport | SC-11, SC-14 |
| [SC-13](#sc-13) | backlog | presets | Presets propios: geometría, packs y bracing separados | SC-11, SC-23 |
| [SC-14](#sc-14) | done | persistencia | Validar flujo existente de proyecto, historial y roundtrip | SC-09, SC-24 |
| [SC-15](#sc-15) | backlog | soportes | Recorridos y colisiones volumétricas de soportes | SC-11, SC-12, SC-38 |
| [SC-16](#sc-16) | backlog | soportes | Minis, parenting y refuerzos editables | SC-15, SC-13 |
| [SC-17](#sc-17) | backlog | soportes | Pipeline de islas y autosupport con resultados editables | SC-48, SC-49, SC-50 |
| [SC-18](#sc-18) | backlog | hollow | Hito hollow 3D y drenajes propios | SC-41, SC-42, SC-43, SC-44, SC-45, SC-46 |
| [SC-19](#sc-19) | backlog | impresion | Salida verificable para la impresora Anycubic elegida | SC-32, SC-35, SC-36, SC-66 |
| [SC-20](#sc-20) | backlog | ui | Hito de pulido integral del editor | SC-25, SC-26, SC-27, SC-28, SC-61 |
| [SC-21](#sc-21) | triage | idea | Explorar FEA aproximada de soportes | SC-53, SC-74 |
| [SC-22](#sc-22) | triage | idea | Evaluar GPU sólo ante cuellos de botella medidos | SC-54 |
| [SC-23](#sc-23) | done | investigacion | Revisión estática de hollow Lychee y conservación local | — |
| [SC-24](#sc-24) | done | calidad | Corpus pequeño y representativo de validación | SC-09 |
| [SC-25](#sc-25) | backlog | editor | Multiselección y operaciones conjuntas | SC-14, SC-28 |
| [SC-26](#sc-26) | backlog | editor | Transformaciones numéricas, restricciones y snapping | SC-14 |
| [SC-27](#sc-27) | backlog | editor | Colocación precisa: centrar, apoyar, alinear y límites | SC-14 |
| [SC-28](#sc-28) | backlog | editor | Jerarquía de escena y propiedades de objeto | SC-14 |
| [SC-29](#sc-29) | backlog | geometria | Robustez de importación, unidades y reparación | SC-24 |
| [SC-30](#sc-30) | backlog | persistencia | Autosave, recuperación de sesión y versiones de proyecto | SC-14, SC-59 |
| [SC-31](#sc-31) | backlog | slicing | Validez de slices persistidos y AA al reabrir | SC-14, SC-33, SC-55 |
| [SC-32](#sc-32) | backlog | perfiles | Perfiles propios e import/export con procedencia | SC-14 |
| [SC-33](#sc-33) | backlog | slicing | Validar sliceado CPU en casos geométricos límite | SC-24 |
| [SC-34](#sc-34) | backlog | slicing | Coherencia visor de capas, raster y exportación | SC-33, SC-35, SC-55 |
| [SC-35](#sc-35) | backlog | impresion | Validar writers existentes y actualizar su catálogo | SC-24, SC-32 |
| [SC-36](#sc-36) | backlog | impresion | Elegir ruta Anycubic y evaluar soluciones existentes | SC-09 |
| [SC-37](#sc-37) | backlog | arquitectura | Contrato común de jobs, cancelación y resultados obsoletos | SC-09, SC-10 |
| [SC-38](#sc-38) | backlog | arquitectura | Cachés derivadas por contenido, revisión y transformaciones | SC-09, SC-24 |
| [SC-39](#sc-39) | backlog | hollow | Especificar contrato hollow 3D y política de espesor | SC-09, SC-23 |
| [SC-40](#sc-40) | backlog | geometria | Preflight de bibliotecas para offset, booleanos y distancia | SC-39, SC-24 |
| [SC-41](#sc-41) | backlog | hollow | Generador de cavidad hollow 3D no destructivo | SC-40, SC-37, SC-38 |
| [SC-42](#sc-42) | backlog | hollow | Espesor físico bajo transformaciones y escala | SC-41, SC-38 |
| [SC-43](#sc-43) | backlog | hollow | Persistencia de parámetros y derivados de hollow | SC-41, SC-14, SC-38 |
| [SC-44](#sc-44) | backlog | hollow | Editor y corte geométrico de drenajes | SC-41, SC-43, SC-40 |
| [SC-45](#sc-45) | backlog | hollow | Blockers y tapones parametrizados de hollow | SC-44, SC-43 |
| [SC-46](#sc-46) | backlog | analisis | Cavidades desconectadas, resina atrapada y ventosas | SC-41, SC-44, SC-33 |
| [SC-47](#sc-47) | triage | hollow | Evaluar hollow 2D como función separada | SC-39, SC-55 |
| [SC-48](#sc-48) | backlog | analisis | Detección de islas por continuidad de capas | SC-33, SC-34 |
| [SC-49](#sc-49) | backlog | soportes | Distribución automática de contactos con exclusiones | SC-48, SC-13, SC-12 |
| [SC-50](#sc-50) | backlog | soportes | Construcción de grafo automático y reparación local | SC-49, SC-15, SC-16 |
| [SC-51](#sc-51) | backlog | geometria | Política de CSG, instancias y bake imprimible | SC-40, SC-14, SC-33 |
| [SC-52](#sc-52) | backlog | geometria | Exportación de pieza, soportes y ensamblaje | SC-12, SC-43, SC-35 |
| [SC-53](#sc-53) | backlog | calidad | Calidad geométrica e imprimibilidad de soportes | SC-11, SC-15, SC-16, SC-24 |
| [SC-54](#sc-54) | backlog | rendimiento | Benchmarks reales de carga, edición y memoria | SC-24, SC-38 |
| [SC-55](#sc-55) | backlog | slicing | Validación raster, AA, pixel pitch y orientación | SC-24, SC-33 |
| [SC-56](#sc-56) | backlog | herramientas | CLI como harness reproducible y paridad con GUI | SC-33, SC-35 |
| [SC-57](#sc-57) | backlog | perfiles | Consumo de resina y tiempo con calibración | SC-32, SC-35, SC-74 |
| [SC-58](#sc-58) | backlog | entrega | Build portable y releases reproducibles de desarrollo | SC-14, SC-59 |
| [SC-59](#sc-59) | backlog | persistencia | Paths, aislamiento de datos y migraciones | SC-09 |
| [SC-60](#sc-60) | backlog | ui | Localización, números, unidades y mensajes propios | SC-26 |
| [SC-61](#sc-61) | backlog | ui | Accesibilidad, foco y descubribilidad de atajos | SC-09 |
| [SC-62](#sc-62) | backlog | documentacion | Mantenimiento de índice, evidencia y board por entrega | SC-08, SC-09 |
| [SC-63](#sc-63) | backlog | mantenimiento | Seguimiento selectivo de upstream sin perder versión propia | SC-08, SC-24 |
| [SC-64](#sc-64) | backlog | calidad | Diagnóstico reproducible y reportes de fallos | SC-37, SC-70 |
| [SC-65](#sc-65) | backlog | calidad | Recuperación ante fallo, cancelación y cierre concurrente | SC-14, SC-37, SC-38 |
| [SC-66](#sc-66) | backlog | impresion | Validación física progresiva con Damian | SC-35, SC-32 |
| [SC-67](#sc-67) | backlog | editor | Orientación asistida y autoarrange | SC-27, SC-46, SC-48 |
| [SC-68](#sc-68) | backlog | geometria | Separar componentes, cortar y edición geométrica | SC-40, SC-51, SC-14 |
| [SC-69](#sc-69) | triage | entrega | Compatibilidad Linux/macOS y empaquetado multiplataforma | SC-58, SC-70 |
| [SC-70](#sc-70) | backlog | plataforma | Compatibilidad GPU, DPI y controles nativos | SC-24 |
| [SC-71](#sc-71) | triage | editor | Múltiples placas y cola de trabajos | SC-14, SC-58 |
| [SC-72](#sc-72) | triage | slicing | Capas adaptativas y perfiles por región | SC-33, SC-35, SC-55 |
| [SC-73](#sc-73) | triage | impresion | Envío a impresora y conectividad de red | SC-19 |
| [SC-74](#sc-74) | backlog | soportes | Calibración experimental de contactos y material | SC-53, SC-66 |
| [SC-75](#sc-75) | backlog | soportes | Re-colocar miembros huérfanos de un host recortado (bug DF) | — |
| [SC-76](#sc-76) | backlog | soportes | Mejoras post-porteo DF: lista de hallazgos | SC-75 |
| [SC-77](#sc-77) | triage | rendimiento | Investigar mover a GPU el pipeline de soportes | SC-76, SC-82 |
| [SC-78](#sc-78) | done | soportes | Fachada del core de soportes DF portados | — |
| [SC-79](#sc-79) | done | soportes | Rebanada vertical GUI: auto-soportes y raft DF de punta a punta | SC-78 |
| [SC-80](#sc-80) | done | slicing | Fallo de stitching de `sl1cer` ("dangling endpoints") | — |
| [SC-81](#sc-81) | done | slicing | Soportes y raft DF en el slice y el archivo de salida | SC-79, SC-80 |
| [SC-82](#sc-82) | paused | rendimiento | Benchmarks del pipeline de soportes en escenas reales | SC-79 |
| [SC-83](#sc-83) | backlog | soportes | Pestaña Supports con cálculo especulativo y caché por etapas | SC-79, SC-37 |
| [SC-84](#sc-84) | in progress | editor | Rediseño de la UI: Transform → Hollow / Supports → Slice → Export | SC-83, SC-18 |
| [SC-85](#sc-85) | backlog | editor | Ayudas visuales del gizmo: grados, distancias y línea del eje | SC-84, SC-26 |
| [SC-86](#sc-86) | backlog | soportes | Panel completo de soportes, lift y presets de Damian | SC-84, SC-83 |
| [SC-87](#sc-87) | backlog | geometria | Repair: mejorar con DF y Caliper como extra | SC-29 |
| [SC-88](#sc-88) | backlog | slicing | Vista de capa con zoom y paneo | SC-84, SC-34 |

## SC-01

**Compilacion nativa y validacion de componentes centrales** — Base verificada / base

Baseline fd238bbd preservada.955 pruebas seleccionadas aprobadas(898unitarias+57espaciales), GUI release y apertura de STL grandes. No suite completa GUI ni impresion validada.

**Cierre:** 955 controles registrados en revision inicial; no reejecutados al organizar.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-02

**Gizmo combinado de mover, rotar y escalar** — Base verificada / editor

Gizmo en Prepare y Transform; Slice solo lectura. Reutiliza implementacion existente; rama selection-gizmo93742b0e preservada.

**Cierre:** Damian confirmo funcionamiento y posteriormente todos los controles.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-03

**Contacto exacto con la cama al rotar y escalar** — Base verificada / editor

Minimo real de vertices world XYZ, cache de bloques256vertices. Mantiene contacto y elevacion intencional; traslacion Z explicita libre. Una transaccion undo.

**Cierre:** 9 controles geometria/timing y62 contrastes de transformaciones; Damian: FUNCIONA PERFECTO.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-04

**Transformaciones modales G/R/S con X/Y/Z** — Base verificada / shortcuts

G mover,R rotar,S escalar; X/Y/Z mundo; Enter/LMB confirma,Esc/RMB cancela. Undo atomico y guardas de revision/seleccion/seccion. No entrada numerica ni Shift+eje aun.

**Cierre:** 8 pruebas matematicas y20 nativas; Damian confirmo funcionamiento.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-05

**Camara Blender, encuadre y perspectiva/ortografica** — Base verificada / camara

MMB orbita,Shift+MMB panea,RMB orbita,rueda zoom.5/boton cambian proyeccion. Picking/render/gizmo coherentes; View Selected y vistas por eje conservan encuadre.

**Cierre:** 22 pruebas camara; Damian confirmo todos los controles.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-06

**Radial con tecla ordinal, swipe y confirmacion al soltar** — Base verificada / camara

Mantener tecla ordinal,mover/resaltar,soltar para ejecutar. Toque permite clic. Fix22526a29: PointerEventKind.move, no moved(drag-only). Umbral central35px. Rama actual experiment/blender-navigation.

**Cierre:** 17/17 nativas:6 vistas delta imagen0, sin accion previa a release; centro,bordes,clic,fuera ventana,25ms swipe,fit,proyeccion,Esc,repeticion. Damian confirmo PERFECTO.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-07

**Revision del informe Lychee y catalogo de presets** — Base verificada / investigacion

Mapa contactos->rutas->geometria->refuerzos.3 SLA fabrica,4 J3D,3packs(2custom),6bracings. Solo investigacion estatica, NO motor portado ni equivalencia de impresion.

**Cierre:** 32 controles independientes;13hashes,888campos,30referencias. Suite original215 no reejecutada.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-08

**Inventario, respaldo privado GitHub e índice mantenible** — Inventario / base

Código aprobado 22526a29; ramas históricas y upstream preservados. GitHub privado SynrgStudio/SC-private, desarrollo sc/development, etiqueta sc-editor-verified-20261022. Release portable y hashes. Commit SC-08 34f81984.

**Cierre:** Ramas remotas y privacidad verificadas; ZIP remoto coincide con SHA256 local; índice, recuperación no destructiva y script de inventario disponibles. No cambios funcionales.

**Depende de:** sin dependencia técnica previa. Prioridad: high. Estado snapshot: done.

## SC-09

**Inventario del core y matriz GUI/core/evidencia/brechas** — Inventario / arquitectura

26 paquetes (20 principal + 6 anidado). Matriz distingue reutilización, integración y validación. Hallazgos: proyectos/reparación/slicing/perfiles ya conectados; soportes demo, Chalyplex vacío; no hollow de producto identificado. Writers GOO3/CTB5/v5enc sí existen.

**Cierre:** Fuentes y hashes trazables, límites de pruebas explícitos; cada brecha mapeada al roadmap. No asumir ausencia por UI ni madurez por tener un tipo/API.

**Depende de:** SC-08. Prioridad: high. Estado snapshot: done.

## SC-10

**Especificar primer hito vertical de soportes manuales** — Siguiente / soportes

Definir colocar contacto, punta/cuerpo/base, seleccionar/editar/borrar, undo, guardar/reabrir y exportar/slicear. Reutilizar scene-edit, picking y jobs existentes; evaluar extensión del contrato sc-geom sin convertir el demo en motor real por nombre.

**Cierre:** Especificación breve con entidades, unidades, estados, cancelación, integración y casos de aceptación; alcance v1 explícito. No programar el hito al cerrar la especificación.

**Depende de:** SC-09. Prioridad: high. Estado snapshot: todo.

## SC-11

**Modelo paramétrico persistente y geometría de soportes** — Desarrollo / soportes

Contacto→recorrido→punta/cuerpo/base como fuente de verdad con IDs y vínculos al modelo; mesh derivada determinista, presets versionados y edición reversible. No copiar fuentes propietarias ni depender del demo hexagonal.

**Cierre:** Geometría válida en casos nominales/límite, serialización roundtrip, migración y cambio de modelo/transform sin soportes huérfanos; meshes derivadas regenerables.

**Depende de:** SC-10, SC-37, SC-38. Prioridad: normal. Estado snapshot: backlog.

## SC-12

**Editor manual de soportes en viewport** — Desarrollo / soportes

Picking de superficie/normal, preview, colocar/seleccionar/mover/borrar, edición individual y cancelación; separar navegación y selección de modelos. Reutilizar renderer y gestos existentes.

**Primera entrega (2026-10-26):** agregar y quitar soportes DF con un clic, en la pestaña Supports.
- **Herramientas:** en el riel, Agregar (A) y Quitar (X). Agregar hace crecer un tronco DF ruteado desde el punto clickeado del modelo, con la normal de esa cara. Quitar borra la entidad más cercana al clic (hasta 3 mm) y todo lo que cuelga de ella, incluida la raíz huérfana.
- **Edición:** el estado de soportes del último Generate queda en `DfSupportSession` (`libs/sc-geom/support-gen-df-sc/src/session.rs`). Cada clic regenera la malla de soportes y la cambia con un solo `ReplaceLeafMesh`, así que es un undo por clic. Undo y Redo siguen editables, porque hay una sesión por revisión de escena. Si la escena cambió por otro motivo, pide regenerar.
- **Pendiente:**
  - mover punta y braces a mano;
  - preview del punto antes del clic;
  - persistir las entidades al guardar (hoy solo sobrevive la malla);
  - la regeneración por clic corre en el hilo de UI, así que hay que medirla en escenas grandes;
  - prueba de gestos de Damian.

**Cierre:** Damian completa flujo manual sin clicks ambiguos; un undo por gesto; cancelación exacta; guardas Slice/loading; soportes sobreviven guardar/reabrir.

**Depende de:** SC-11, SC-14. Prioridad: normal. Estado snapshot: in progress.

## SC-13

**Presets propios: geometría, packs y bracing separados** — Desarrollo / presets

Unidades, rangos, esquemas y migración; biblioteca de presets personales editable/importable. Referencias J3D como datos; bp0 ausente y joinLength ambiguo no se normalizan a ciegas.

**Cierre:** Crear/duplicar/editar/importar/exportar/restaurar; dependencias faltantes visibles; cambiar preset no altera objetos existentes sin acción explícita.

**Depende de:** SC-11, SC-23. Prioridad: normal. Estado snapshot: backlog.

## SC-14

**Validar flujo existente de proyecto, historial y roundtrip** — Siguiente / persistencia

Guardar/abrir 3MF YA existe: probar importar varios modelos, transformar, copiar/pegar/borrar, reparar, undo/redo, perfiles embebidos y exportación. Comparar geometría/transform/identidad y estado dirty. Implementar sólo fallos demostrados.

**Cierre:** Corpus roundtrip sin pérdidas de geometría, escala, pose ni perfiles; guardado atómico, save-as, cancelación y archivo inválido controlados; evidencia exacta de build.

**Depende de:** SC-09, SC-24. Prioridad: high. Estado snapshot: done.

**Cierre SC-14:** 14 controles nativos iniciales + 9 de la build corregida; 126 tests focales. Corregida pérdida de cambios al abrir/cerrar dirty. Release separada y baseline aprobada preservada. Clippy estricto conserva deuda anterior (6 diagnósticos); no suite completa ni impresión física. [Informe](PROJECT-FLOW.md).

## SC-15

**Recorridos y colisiones volumétricas de soportes** — Desarrollo / soportes

Vertical antes de inclinado/model-to-model. Contrato de clearance, cuello, longitudes y restricciones. Aceleración espacial existente; rayos de muestreo no certifican ausencia de intersección volumétrica.

**Cierre:** Fixtures cóncavos, paredes finas, huecos, vecinos y escalas; diagnóstico de trayecto imposible; edición/undo conserva anclajes.

**Depende de:** SC-11, SC-12, SC-38. Prioridad: normal. Estado snapshot: backlog.

## SC-16

**Minis, parenting y refuerzos editables** — Desarrollo / soportes

Grafo de padres/ramas y arriostramientos parametrizados; bases sobre modelo, uniones, patrones y regeneración local. No tratar fuerza heurística como garantía física.

**Cierre:** Sin ciclos/huérfanos; modificar/eliminar padre resuelve dependencias; joints y cruces imprimibles; roundtrip/undo y selección claros.

**Depende de:** SC-15, SC-13. Prioridad: normal. Estado snapshot: backlog.

## SC-17

**Pipeline de islas y autosupport con resultados editables** — Desarrollo / soportes

Hito integrador de SC-48/49/50: usar contornos por capa, categorías y generador propio de contactos/rutas. Preview de propuestas, aceptar parcial, preservar manuales y registrar seed/config.

**Cierre:** Cobertura, falsos positivos, tiempo y memoria medidos en corpus; no duplicar contactos existentes; resultados editables y reproducibles; sin prometer paridad Lychee.

**Depende de:** SC-48, SC-49, SC-50. Prioridad: normal. Estado snapshot: backlog.

## SC-18

**Hito hollow 3D y drenajes propios** — Desarrollo / hollow

Hito integrador, no duplicar subtareas SC-39…46. Fuente paramétrica persistente, cavidad mesh, agujeros, blockers, tapones, preview/export/slice coherentes. No portar a ciegas DF ni interpretar descripción sc-geom como algoritmo.

**Cierre:** Flujo no destructivo completo y comprobable, espesor físico y fallos explícitos, sin volúmenes atrapados no advertidos; guardado/reapertura e historial.

**Base acordada (2026-10-26):** portar el hollow de DragonFruit, la versión que Damian ajustó. Detalle del motor y de sus aportes en SC-84, sección Relevamiento:
- motor: vóxeles con distancia chamfer, cavidad suavizada, estabilización manifold, infill lattice/pillars, áreas sólidas, drenajes con profundidad automática;
- aportes de Damian: grilla local al modelo, preview separada del Apply, voxelizado paralelo ~5×.

Corregir lo flojo: vóxel efectivo visible, progreso y cancelación, resultado en ml.

**Depende de:** SC-41, SC-42, SC-43, SC-44, SC-45, SC-46. Prioridad: normal. Estado snapshot: backlog.

## SC-19

**Salida verificable para la impresora Anycubic elegida** — Desarrollo / impresion

Primero confirmar modelo/firmware y formato real con Damian. GOO/CTB existentes no implican compatibilidad Anycubic. Decidir writer existente, extensión o puente UVtools según viabilidad y licencia.

**Cierre:** Archivo independiente validado en dimensiones, capas, orientación, exposición y movimiento; aceptación por firmware y prueba física realizadas por Damian antes de declarar producción.

**Depende de:** SC-32, SC-35, SC-36, SC-66. Prioridad: normal. Estado snapshot: backlog.

## SC-20

**Hito de pulido integral del editor** — Desarrollo / ui

Agrupa selección, inspector, transformaciones, jerarquía, colocación y accesibilidad. Priorizar con uso real; las bases ya probadas se conservan. Subtareas SC-25…28,60/61/70.

**Cierre:** Flujo cotidiano consistente, atajos descubribles, sin regresión de navegación/modales; cada mejora tiene evidencia y no exige rehacer renderer.

**Depende de:** SC-25, SC-26, SC-27, SC-28, SC-61. Prioridad: normal. Estado snapshot: backlog.

## SC-21

**Explorar FEA aproximada de soportes** — Ideas / idea

Red de vigas con cargas de separación/material/proceso calibrados. Investigación opcional; no bloquear editor/manual/impresión ni presentar mapa de colores como validación mecánica.

**Cierre:** Spike con coste, incertidumbre, correlación experimental y decisión continuar/descartar; sin nueva dependencia de pago.

**Depende de:** SC-53, SC-74. Prioridad: low. Estado snapshot: triage.

## SC-22

**Evaluar GPU sólo ante cuellos de botella medidos** — Ideas / idea

Medir CPU, transferencias, VRAM y latencia antes de migrar una etapa; distinguir raster/render ya GPU de geometría computacional.

**Cierre:** Benchmark reproducible, precisión y fallback CPU; decisión fundada. No migración general por intuición.

**Depende de:** SC-54. Prioridad: low. Estado snapshot: triage.

## SC-23

**Revisión estática de hollow Lychee y conservación local** — Investigación / investigacion

Informe/parámetros/auditoría leídos previamente; 336 archivos cotejados. Registro original 1504 estáticas solapadas +81 auditoría, no reejecutado. 2D≠3D, diferencias preview/export/slice, escala/cachés y diámetro reducido documentados.

**Cierre:** Referencia y límites en RESEARCH.md; backup local fuera de Temp con manifiesto, sin subir bundles/extractos propietarios a GitHub. No es motor implementado.

**Depende de:** sin dependencia técnica previa. Prioridad: normal. Estado snapshot: done.

## SC-24

**Corpus pequeño y representativo de validación** — Siguiente / calidad

Fixtures sintéticos/libres: sólidos, huecos, paredes finas, concavidades, grupos, instancias, unidades, escalas negativas/no uniformes y malla rota. Casos grandes sólo locales si tienen datos privados.

**Cierre:** Procedencia, licencia, hash, unidades y expectativas por fixture; distinguir checks geométricos, imágenes y pruebas físicas. Reutilizar mesh/scene-fixtures existentes.

**Depende de:** SC-09. Prioridad: high. Estado snapshot: done.

**Entrega:** 25 casos sintéticos, 28 assets (27,045 bytes), manifiesto de procedencia/licencia/unidades/hash/expectativas, generador Rust que reutiliza fixtures/APIs existentes y verificador Python. 18 casos geométricos y 13 sondas analíticas; dos generaciones idénticas byte a byte. No GUI, raster completo ni impresión física.

[Corpus](../../fixtures/sc-corpus/README.md) · [Verificación](CORPUS-VERIFICATION.json).

## SC-25

**Multiselección y operaciones conjuntas** — Desarrollo / editor

Selection actual es None/One. Extender selección, caja/Shift, copy/paste/delete/duplicate, pivote y transforms conjuntas sin perder IDs ni coherencia de undo.

**Cierre:** Selección visible y predecible, un undo por operación, scene list y viewport coinciden; recursos compartidos no se duplican sin necesidad.

**Depende de:** SC-14, SC-28. Prioridad: normal. Estado snapshot: backlog.

## SC-26

**Transformaciones numéricas, restricciones y snapping** — Desarrollo / editor

Entrada numérica G/R/S, precisión Shift, planos excluidos, incrementos, world/local y pivotes con HUD claro. Mantener Esc/confirm, foco y contacto cama existentes.

**Cierre:** Unidades mm/grados/factor claras; valores no válidos no mutan escena; cancel restaura exactamente; testeos focales y Damian prueba gestos.

**Pedido de Damian (2026-10-26):** al seleccionar un objeto, ver sus medidas reales y mover, rotar y escalar con números, viendo cuánto se movió o rotó; "todos los controles básicos". Va en el inspector de Transform (SC-84), con la tarjeta de la herramienta activa abierta. Prioridad: high.

**Primera entrega (2026-10-26):** tarjetas MOVE, ROTATE y SIZE en el inspector de Transform (`viewport/numeric_transform.rs`, `NumberField`).
- **Valores:** posición (centro de la huella en X/Y y lift en Z), rotación XYZ en grados, tamaño real del objeto en mm y escala en %.
- **Edición:** cada valor tipeado es una edición deshacible.
- **Física:** rotar y escalar giran alrededor del centro de la huella y conservan el lift; el tamaño escala uniforme.
- **Botones:** "Drop to plate" y "Center".
- **Rendimiento:** la lectura se calcula como mucho una vez por frame. Durante un gesto solo se leen la matriz y el delta de traslación; la caja ajustada se recalcula al soltar, así el arrastre no recorre la malla en cada tick.
- **Tests:** controller (lift con undo, gating fuera de Transform, alto uniforme, rotación con lift) y Euler ida y vuelta.

Falta: escala no uniforme por eje, world/local, snapping, entrada numérica dentro del gesto G/R/S y la prueba de Damian.

**Depende de:** SC-14. Prioridad: normal. Estado snapshot: backlog.

## SC-27

**Colocación precisa: centrar, apoyar, alinear y límites** — Desarrollo / editor

Reutilizar print-glue placement y mínimo de vértices. Centrar, drop-to-bed, elevación intencional, alineación y avisos fuera de volumen; aclarar política ante escalas invertidas.

**Cierre:** Dimensiones/posición correctas y un undo; no corregir silenciosamente Z intencional; avisos coinciden con exportación.

**Pedido de Damian (2026-10-26):** lift del objeto (también en Supports, SC-86), apoyar en cara y centrar, en el riel de Transform. Prioridad: high.

**Depende de:** SC-14. Prioridad: normal. Estado snapshot: backlog.

## SC-28

**Jerarquía de escena y propiedades de objeto** — Desarrollo / editor

El core tiene grupos/recursos pero la UI lista hojas. Definir árbol, nombres, grupos, visibilidad, bloqueo, selección y qué estado es sólo visual vs imprimible.

**Cierre:** Identidades y jerarquía sobreviven roundtrip; operaciones no borran recursos compartidos; ocultar no cambia export sin política explícita.

**Depende de:** SC-14. Prioridad: normal. Estado snapshot: backlog.

## SC-29

**Robustez de importación, unidades y reparación** — Desarrollo / geometria

Extender/validar readers y repair existentes, no reescribir. Límites, ASCII/binary STL, OBJ multiobjeto, 3MF, winding, degenerados y diagnóstico antes/después.

**Cierre:** No modifica originales; informe de cambios y pérdida, fallo/cancel controlados; unidades explícitas en formatos sin unidad; fixtures adversos.

**Depende de:** SC-24. Prioridad: normal. Estado snapshot: backlog.

## SC-30

**Autosave, recuperación de sesión y versiones de proyecto** — Desarrollo / persistencia

Guardar proyecto existente no es autosave. Snapshots atómicos con límites/retención, recuperación tras crash y no sobrescribir fuente ni perfil del usuario por accidente.

**Cierre:** Interrupción simulada recuperable, sin confundir dirty con guardado; elección de recuperar/descartar y limpieza acotada; compatible con migraciones.

**Depende de:** SC-14, SC-59. Prioridad: normal. Estado snapshot: backlog.

## SC-31

**Validez de slices persistidos y AA al reabrir** — Desarrollo / slicing

Auditar fingerprint/reloaded_slices: AA no está persistido ni en token y vuelve Off según código actual. Decidir qué conserva contours vs raster y comunicar diferencias sin alarmas falsas.

**Cierre:** Cambios de mesh/pose/perfil/AA tienen política correcta y probada; no reutilizar raster obsoleto; roundtrip de configuración explícito.

**Depende de:** SC-14, SC-33, SC-55. Prioridad: normal. Estado snapshot: backlog.

## SC-32

**Perfiles propios e import/export con procedencia** — Desarrollo / perfiles

Reutilizar print-profiles/forms/libraries. Definir impresora real, materiales, herencia, overrides y defaults visibles; paquetes y perfiles embebidos sin sobrescrituras inesperadas.

**Cierre:** Resolver perfil efectivo y origen por campo; validación de rangos y migraciones; unidades/resolución/mirror correctos. Ajustes físicos confirmados por Damian.

**Depende de:** SC-14. Prioridad: normal. Estado snapshot: backlog.

## SC-33

**Validar sliceado CPU en casos geométricos límite** — Desarrollo / slicing

Contornos, orientación de holes, capas inicial/final, objetos superpuestos, alturas, manifold/cancel y determinismo; reutilizar engine y servicios existentes.

**Cierre:** Contornos y schedules esperados en corpus, cancel sin resultado parcial instalado; límites CSG/reference reportados, no silenciados.

**Depende de:** SC-24. Prioridad: normal. Estado snapshot: backlog.

## SC-34

**Coherencia visor de capas, raster y exportación** — Desarrollo / slicing

Conectar verificación de rutas existentes: sección 3D, contornos, capas raster, printer program y export. Comprobar Z de muestreo frente techo de slab, orientación y píxeles.

**Cierre:** Fixtures con resultados cruzados documentados; tocar perfil/escena invalida sólo lo debido; no llamar equivalentes a previews distintos.

**Depende de:** SC-33, SC-35, SC-55. Prioridad: normal. Estado snapshot: backlog.

## SC-35

**Validar writers existentes y actualizar su catálogo** — Desarrollo / impresion

Contrastar write.rs/has_writer con UI/docs: GOO3.0, CTB5 y CTB5enc implementados; GOO5.1 rechazado. Verificar bytes/metadata, thumbnails, límites y fallos de I/O.

**Cierre:** Roundtrip/lector independiente o fixture oficial por dialecto; README y UI reflejan soporte exacto; archivo generado no implica prueba física.

**Depende de:** SC-24, SC-32. Prioridad: normal. Estado snapshot: backlog.

## SC-36

**Elegir ruta Anycubic y evaluar soluciones existentes** — Decisión pendiente de equipo / impresion

Identificar equipo/firmware/formato; revisar opciones abiertas mantenidas y UVtools, licencias/coste cero y límites. Sólo después decidir adaptación o writer propio.

**Cierre:** Decisión documentada con muestra válida y plan de validación independiente; no instalar ni comprar servicios por la investigación.

**Depende de:** SC-09. Prioridad: normal. Estado snapshot: backlog.

## SC-37

**Contrato común de jobs, cancelación y resultados obsoletos** — Desarrollo / arquitectura

Inventariar workers actuales; ampliar reusable JobHandle/revision/diagnósticos/recipe. Evitar trabajos pesados en gestos y mutaciones después de cambiar de escena.

**Cierre:** Cancelar/cerrar/editar/reabrir durante operación no instala resultados viejos; progreso y errores recuperables; commit de resultado atómico con undo.

**Depende de:** SC-09, SC-10. Prioridad: normal. Estado snapshot: backlog.

## SC-38

**Cachés derivadas por contenido, revisión y transformaciones** — Desarrollo / arquitectura

AABB/BVH/vertex-blocks/GPU/soportes/hollow/slices: dependencias explícitas, priming off-thread, presupuestos y compartir recursos inmutables sin estado global obsoleto.

**Cierre:** Edits/undo/repair/scale invalidan sólo lo afectado; resultados coinciden con cálculo sin caché en casos pequeños; coste frío/caliente medido.

**Depende de:** SC-09, SC-24. Prioridad: normal. Estado snapshot: backlog.

## SC-39

**Especificar contrato hollow 3D y política de espesor** — Desarrollo / hollow

Diseño de fuente mesh+cavidad derivada, unidades, calidad/error, escala, export/bake, blockers y fallos. Separar futuro raster2D. Requisitos de investigación, no réplica de implementación Lychee.

**Cierre:** Decisiones explícitas de espacio local/world, espesor tras scale, preview vs final y límites; criterios geométricos y memoria antes de código.

**Depende de:** SC-09, SC-23. Prioridad: normal. Estado snapshot: backlog.

## SC-40

**Preflight de bibliotecas para offset, booleanos y distancia** — Desarrollo / geometria

Comparar soluciones abiertas mantenidas para SDF/offset/isosuperficie/booleanos y posibilidad Rust/FFI. Revisar licencias, robustez, fallback, build Windows y coste de memoria.

**Cierre:** Spike acotado con fixtures y decisión de reutilizar/crear, sin nueva plataforma paga. No integrar bundles propietarios ni suponer solución universal.

**Depende de:** SC-39, SC-24. Prioridad: normal. Estado snapshot: backlog.

## SC-41

**Generador de cavidad hollow 3D no destructivo** — Desarrollo / hollow

Implementar motor elegido con calidad/error y memoria acotados; mesh interior con winding correcto, tratamiento de degenerados y worker cancelable.

**Cierre:** Medir espesor/volumen y topología en corpus; fallos explícitos, preview rápido vs final etiquetados; input original intacto.

**Depende de:** SC-40, SC-37, SC-38. Prioridad: normal. Estado snapshot: backlog.

## SC-42

**Espesor físico bajo transformaciones y escala** — Desarrollo / hollow

Definir regeneración ante escala uniforme/no uniforme, rotación, mirror, pivotes y bake; evitar timestamp de UI como validez geométrica.

**Cierre:** Espesor en mm cumple tolerancia declarada después de transform/undo/reopen; caché vieja nunca aparenta resultado actual.

**Depende de:** SC-41, SC-38. Prioridad: normal. Estado snapshot: backlog.

## SC-43

**Persistencia de parámetros y derivados de hollow** — Desarrollo / hollow

Extender proyecto 3MF con versión/migración, parámetros, holes/blockers y política de cache/bake. Original recuperable; IDs y dependencias estables.

**Cierre:** Roundtrip y compatibilidad hacia atrás, aviso de datos desconocidos sin pérdida silenciosa; un undo por operación y regeneración reproducible.

**Depende de:** SC-41, SC-14, SC-38. Prioridad: normal. Estado snapshot: backlog.

## SC-44

**Editor y corte geométrico de drenajes** — Desarrollo / hollow

Colocación por superficie/normal, diámetro/profundidad/inclinación; preview vs corte final, exterior/interior y casos de múltiples paredes. Diferenciar fallo, nominal y diámetro efectivo.

**Cierre:** Mesh sellada en casos soportados; agujero atraviesa pared prevista; diámetro no se reduce silenciosamente; editar/borrar/undo y persistencia.

**Depende de:** SC-41, SC-43, SC-40. Prioridad: normal. Estado snapshot: backlog.

## SC-45

**Blockers y tapones parametrizados de hollow** — Desarrollo / hollow

Mantener regiones macizas mediante blockers y generar tapones con holgura física independiente; edición, orientación, in-place vs pieza aparte y exportación clara.

**Cierre:** Transformaciones y tolerancias verificadas; no obstruir drenajes sin aviso; objetos separados identificables y roundtrip completo.

**Depende de:** SC-44, SC-43. Prioridad: normal. Estado snapshot: backlog.

## SC-46

**Cavidades desconectadas, resina atrapada y ventosas** — Desarrollo / analisis

Componentes de cavidad, conectividad a exterior/agujeros, mínimos de apertura y análisis de copas por dirección/capas. Heurísticas explícitas, no garantía mecánica.

**Cierre:** Fixtures positivos/negativos y representación útil en viewport/capas; advertencia distingue detección, incertidumbre y acción recomendada.

**Depende de:** SC-41, SC-44, SC-33. Prioridad: normal. Estado snapshot: backlog.

## SC-47

**Evaluar hollow 2D como función separada** — Ideas / hollow

Opcional: erosión/raster por capas con espesor XY/Z y patrones. No genera cavidad STL por sí solo; distinguir AA/dimming de volumen de relleno.

**Cierre:** Contrato y comparación preview/export en píxeles físicos, límites anisotrópicos y drenajes; no presentarlo como hollow3D.

**Depende de:** SC-39, SC-55. Prioridad: low. Estado snapshot: triage.

## SC-48

**Detección de islas por continuidad de capas** — Desarrollo / analisis

Aprovechar contours/raster para componentes nuevos, microislas, enlaces entre capas y umbrales físicos; visualización localizada y navegación a capa.

**Cierre:** Corpus etiquetado, cobertura/falsos positivos medidos, resultados estables ante AA/resolución y cancelación; sin confundir overhang con isla.

**Depende de:** SC-33, SC-34. Prioridad: normal. Estado snapshot: backlog.

## SC-49

**Distribución automática de contactos con exclusiones** — Desarrollo / soportes

Categorías isla/overhang/detalle, separación física, áreas enforcers/blockers y gradientes. Usar presets separados y preservar contactos manuales.

**Cierre:** Seed/config reproducibles; densidad/separación sin unidad ambigua; propuesta revisable y regeneración selectiva sin destruir trabajo.

**Depende de:** SC-48, SC-13, SC-12. Prioridad: normal. Estado snapshot: backlog.

## SC-50

**Construcción de grafo automático y reparación local** — Desarrollo / soportes

Resolver contactos hacia cama/modelo/ramas, colisiones y bracing con costes explícitos. Reroute local tras edición, no reconstrucción global por cada movimiento.

**Cierre:** Contactos imposibles diagnosticados, sin ciclos ni rutas ocultamente inválidas; reproducible, cancelable y editable tras aceptar.

**Depende de:** SC-49, SC-15, SC-16. Prioridad: normal. Estado snapshot: backlog.

## SC-51

**Política de CSG, instancias y bake imprimible** — Desarrollo / geometria

Scene representa CSG/reference pero slicing los rechaza. Definir qué evaluar/expandir/bakear, límites de explosión y origen conservado para undo.

**Cierre:** Vista, export y slice coinciden en casos admitidos; casos no admitidos se rechazan claramente; no asumir booleano por tener enum.

**Depende de:** SC-40, SC-14, SC-33. Prioridad: normal. Estado snapshot: backlog.

## SC-52

**Exportación de pieza, soportes y ensamblaje** — Desarrollo / geometria

Selección/todo, STL plano vs OBJ/3MF estructurado, soportes y hollow incluidos según opción, bake explícito y warnings de pérdida.

**Cierre:** Dimensiones/posición/topología verificadas con lector independiente; malla exportada coincide con preview final y no cambia proyecto fuente.

**Depende de:** SC-12, SC-43, SC-35. Prioridad: normal. Estado snapshot: backlog.

## SC-53

**Calidad geométrica e imprimibilidad de soportes** — Desarrollo / calidad

Punta, cuello, cuerpo, pie, juntas y refuerzos: watertight donde corresponde, intersecciones útiles, sección mínima y contacto/pentración controlados.

**Cierre:** Fixtures y límites de tamaño/radio probados; mediciones guardadas por preset; no equiparar manifold a suficiente fuerza de impresión.

**Depende de:** SC-11, SC-15, SC-16, SC-24. Prioridad: normal. Estado snapshot: backlog.

## SC-54

**Benchmarks reales de carga, edición y memoria** — Desarrollo / rendimiento

Separar parse/prep/primera imagen/picking/gestos/geometry jobs/export, frío/caliente y memoria CPU/VRAM. No usar captura de pantalla como FPS.

**Cierre:** Corpus y hardware/config registrados, presupuestos comparables antes/después; detectar regresiones sin suites largas por cada cambio trivial.

**Depende de:** SC-24, SC-38. Prioridad: normal. Estado snapshot: backlog.

## SC-55

**Validación raster, AA, pixel pitch y orientación** — Desarrollo / slicing

AA Off/2/4/8, mirror/rotación, umbral de exposición y resolución anisotrópica; comparar contornos con máscara física y capas de writer.

**Cierre:** Fixtures de paredes/detalles/holes con resultados pixel conocidos; AA cambia cobertura sin mover dimensiones nominales fuera de tolerancia.

**Depende de:** SC-24, SC-33. Prioridad: normal. Estado snapshot: backlog.

## SC-56

**CLI como harness reproducible y paridad con GUI** — Desarrollo / herramientas

Reusar slice/convert/diagnose/fix/profiles existentes y servicios core. Añadir casos de batch/regresión y geometría sólo según funciones reales.

**Cierre:** Mismo input/config produce resultado equivalente; códigos de salida/errores/progreso y rutas con espacios correctos; sin duplicar algoritmo GUI.

**Depende de:** SC-33, SC-35. Prioridad: normal. Estado snapshot: backlog.

## SC-57

**Consumo de resina y tiempo con calibración** — Desarrollo / perfiles

Reutilizar volumen y printer_time_calibration; separar estimación geométrica, pérdidas, soportes y schedule mecánico. Datos medidos por equipo/resina.

**Cierre:** Unidades y supuestos visibles, rangos/error frente a mediciones reales; no presentar demora demo ni tiempo CPU como tiempo de impresión.

**Depende de:** SC-32, SC-35, SC-74. Prioridad: normal. Estado snapshot: backlog.

## SC-58

**Build portable y releases reproducibles de desarrollo** — Desarrollo / entrega

Extender packages/windows, incluir perfiles/licencias/versión/hash y notas. CI focal, firma sólo si se decide; no asociaciones ni instalador obligatorio para probar.

**Cierre:** Artefacto de checkout limpio abre fixtures sin rutas del workspace; manifest de build y rollback; no subir modelos privados ni claves.

**Depende de:** SC-14, SC-59. Prioridad: normal. Estado snapshot: backlog.

## SC-59

**Paths, aislamiento de datos y migraciones** — Desarrollo / persistencia

app-paths usa rutas del OS; definir namespace de nuestra versión, perfiles y portabilidad sin depender de APPDATA override no efectivo. No tocar Lychee/DF.

**Cierre:** Ejecución de desarrollo/portable/normal con datos separados donde se prometa; migración reversible y compatibilidad de perfiles documentadas.

**Depende de:** SC-09. Prioridad: normal. Estado snapshot: backlog.

## SC-60

**Localización, números, unidades y mensajes propios** — Desarrollo / ui

Reutilizar message-catalog/i18n; auditar HUD y nuevos textos. Separador decimal, formato/unidades, caracteres/teclados y errores orientados a acción.

**Cierre:** Sin cadenas rotas/mojibake, traducciones no fingen cobertura; edición numérica consistente con locale y atajos probados en teclado de Damian.

**Depende de:** SC-26. Prioridad: normal. Estado snapshot: backlog.

## SC-61

**Accesibilidad, foco y descubribilidad de atajos** — Desarrollo / ui

Ayuda de G/R/S, radial/proyección, tooltips, foco teclado y contraste; densidad/inspector, carga/progreso/errores sin bloquear interacción necesaria.

**Cierre:** Acciones descubribles sin memorizar instrucciones; Esc/Enter no actúan sobre control equivocado; navegación por teclado usable y Damian valida.

**Depende de:** SC-09. Prioridad: normal. Estado snapshot: backlog.

## SC-62

**Mantenimiento de índice, evidencia y board por entrega** — Continuo / documentacion

Rutina continua, no daemon: al cerrar cambios actualizar matriz/INVENTORY/roadmap, commit/build hash y tarjeta. Board es operativo; snapshot Git es respaldo, no fuente paralela de estado.

**Cierre:** Cada entrega enlaza código/artefacto/prueba y límites; snapshots fechados y sin tareas duplicadas. No marcar esta rutina perpetua como terminada por documentarla.

**Depende de:** SC-08, SC-09. Prioridad: normal. Estado snapshot: backlog.

## SC-63

**Seguimiento selectivo de upstream sin perder versión propia** — Continuo / mantenimiento

Origin Codeberg separado de github; revisar cambios con diff y pruebas focales antes de cherry-pick/merge. Mantener licencias/autoría y baseline propio.

**Cierre:** Cada adopción documenta beneficio/conflictos/regresiones; no auto-sync, PRs públicos ni cambios en DF por esta tarjeta.

**Depende de:** SC-08, SC-24. Prioridad: normal. Estado snapshot: backlog.

## SC-64

**Diagnóstico reproducible y reportes de fallos** — Desarrollo / calidad

Errores de import/geometry/GPU/export con versión/hash/config y logs acotados. Reporte local voluntario, sin telemetría o subir escenas automáticamente.

**Cierre:** Un fallo se puede reproducir con fixture sintético y logs sin secretos/modelos; UI muestra recuperación y ubicación del reporte.

**Depende de:** SC-37, SC-70. Prioridad: normal. Estado snapshot: backlog.

## SC-65

**Recuperación ante fallo, cancelación y cierre concurrente** — Desarrollo / calidad

Inyección de fallos de I/O, falta de memoria, archivo corrupto y cierre/cambio de escena en jobs. Reutilizar gates/stamps en lugar de callbacks ad hoc.

**Cierre:** Estado consistente, archivos destino intactos ante rechazo y undo no corrompido; no resultados obsoletos ni app colgada al cancelar.

**Depende de:** SC-14, SC-37, SC-38. Prioridad: normal. Estado snapshot: backlog.

## SC-66

**Validación física progresiva con Damian** — Validación con Damian / impresion

Plan de cubos/dimensiones, detalles, soportes, hollow y drenajes. Validación digital primero; Damian ejecuta pruebas en máquina y confirma resultados reales.

**Cierre:** Registro equipo/firmware/resina/exposición/capas/medidas y criterio pasa/falla; generar archivo no cierra la tarea física. No iniciar impresoras automáticamente.

**Depende de:** SC-35, SC-32. Prioridad: normal. Estado snapshot: backlog.

## SC-67

**Orientación asistida y autoarrange** — Desarrollo / editor

Extensión opcional sobre placement: separación, volumen útil y restricciones, estimar área por capa/altura/contactos/ventosas antes de sugerir orientación.

**Cierre:** Previsualizar/aceptar/undo, no mover trabajo manual sin acción; métricas y límites explícitos, tiempo/memoria medidos.

**Depende de:** SC-27, SC-46, SC-48. Prioridad: low. Estado snapshot: backlog.

## SC-68

**Separar componentes, cortar y edición geométrica** — Desarrollo / geometria

Opcional: split por componentes, plano de corte, caps y merge/bake; evaluar soluciones existentes primero. Conservar original e historial y relaciones de soportes.

**Cierre:** Mesh válida en casos admitidos, referencias migradas o invalidadas explícitamente; roundtrip y no confundir merge con unión booleana.

**Depende de:** SC-40, SC-51, SC-14. Prioridad: low. Estado snapshot: backlog.

## SC-69

**Compatibilidad Linux/macOS y empaquetado multiplataforma** — Ideas / entrega

Sólo cuando se quiera usar esas plataformas: validar builds/paths/renderer/fonts/input y packaging existente; no asumir probado por compilar Windows.

**Cierre:** Prueba nativa por plataforma y backend, límites documentados; no instalar toolchains globales sin necesidad concreta.

**Depende de:** SC-58, SC-70. Prioridad: low. Estado snapshot: triage.

## SC-70

**Compatibilidad GPU, DPI y controles nativos** — Desarrollo / plataforma

Renderer wgpu/Slint, fallbacks, resize, monitores/DPI, teclado ordinal/layout, pérdida de foco y device loss. Conservar la semántica hover correcta.

**Cierre:** Arranque y errores claros en hardware disponible, gestos sin regresión en DPI/foco; pruebas focales ante fallos, no suite de mouse extensa rutinaria.

**Depende de:** SC-24. Prioridad: normal. Estado snapshot: backlog.

## SC-71

**Múltiples placas y cola de trabajos** — Ideas / editor

Idea posterior: documentos/placas independientes, perfiles por placa, duplicados compartidos de forma segura y batch export con progreso.

**Cierre:** Persistencia e historial por placa, cancelación y nombres de salida sin sobrescritura; no confundir con controlar impresora por red.

**Depende de:** SC-14, SC-58. Prioridad: low. Estado snapshot: triage.

## SC-72

**Capas adaptativas y perfiles por región** — Ideas / slicing

Investigación posterior: engine actual usa schedule uniforme con primera capa propia; evaluar coste en raster, writers, preview, fingerprint y perfil de exposición.

**Cierre:** Prototipo con beneficio medido y compatibilidad de formato; no tocar alturas sin coherencia integral ni prometer ahorro universal.

**Depende de:** SC-33, SC-35, SC-55. Prioridad: low. Estado snapshot: triage.

## SC-73

**Envío a impresora y conectividad de red** — Ideas / impresion

Idea separada de generación de archivo. Identificar protocolo del equipo y plugin/proyecto mantenido; credenciales y control explícito del usuario.

**Cierre:** Conexión/envío/cancel/status verificados si se implementa; imprimir requiere acción expresa de Damian; no habilitar servicios por el roadmap.

**Depende de:** SC-19. Prioridad: low. Estado snapshot: triage.

## SC-74

**Calibración experimental de contactos y material** — Validación con Damian / soportes

Plan de probetas con radios/penetración/altura/bracing y registros por resina/exposición/impresora/temperatura. Medir ruptura, marcas y estabilidad sin asumir presets universales.

**Cierre:** Resultados físicos de Damian ligados a preset/version/config; recomendaciones con rango e incertidumbre, nunca garantía de resistencia.

**Depende de:** SC-53, SC-66. Prioridad: normal. Estado snapshot: backlog.

## SC-75

**Re-colocar miembros huérfanos de un host recortado (bug DF)** — Mejoras post-porteo / soportes

Hallado al portar `autoPlace.ts` (bloque 6c, `docs/sc/DF-PORT.md`): DF filtra los huérfanos a re-colocar con `'host trunk culled'`, pero el cull escribe `'trunk culled (blocked)'`, así que leaves/branches de un trunk recortado por atravesar la malla nunca se re-colocan. Arreglo propuesto: marcar el caso con un dato (`OrphanReason::HostCulled` o `host_culled`), re-encolar con área/altura/preset y origen reales del ledger en vez de `islandAreaMm2: 0.05`, re-correr el resize del bosque tras re-colocar, un único recheck sin re-encolar y contadores de recuperados/no recuperados en la analítica. Va detrás de una opción del plan: activada en SC, desactivada en el test de paridad, que sigue comparando bit a bit con DF.

**Cierre:** Test propio sobre los casos del fixture 6c (6 miembros en 3 corridas) que termina con esos miembros soportados, sin colisión ni huérfanos, y cobertura no menor que DF; paridad DF intacta con la opción apagada; revisión visual de Damian cuando esté en la GUI. No tocar el código de referencia de DF.

**Depende de:** porteo DF completo (rama `sc/df-supports`). Prioridad: normal. Estado snapshot: backlog.

## SC-76

**Mejoras post-porteo DF: lista de hallazgos** — Mejoras post-porteo / soportes

Lista viva de "esto podría estar mejor" anotada durante el porteo (`docs/sc/DF-PORT.md` tiene el detalle por bloque). Nada se aplica hasta terminar el porteo; cada arreglo va detrás de una opción, con la paridad DF intacta al apagarla.

- **Kickstands nuevos sin brazos (bug DF, bloque 7):** `buildKickstandData` no estampa `typeId`, y `resolveSegmentEndpoints` resuelve el tipo por el store; un kickstand generado en la misma pasada no está ahí, así que nunca se le colocan brazos en la pasada que lo crea. Arreglo: estampar `typeId` en el builder.
- **Raíces y knots huérfanos al regenerar kickstands (bloque 7):** los kickstands auto-generados se descartan en cada pasada, pero sus raíces y knots quedan en el snapshot, porque el resultado se arma desde el snapshot original. Arreglo: soltar lo que solo reclamaba el kickstand descartado.
- **`removedBraceCount` engañoso (bloque 7):** cuenta todos los brazos previos, también los que se conservan. Arreglo: contar solo los quitados.
- **Orden dependiente del locale (bloque 7):** `sortSupports` usa `localeCompare` (colación ICU `en-US`); otro locale podría cambiar el orden y el bracing. Arreglo: orden por unidades de código, que es determinista.
- **Gap-fill lee el store comprometido, no el borrador (bloque 6c):** los soportes de la corrida no cuentan como cobertura; DF lo deja así a propósito y dice que con el borrador converge bien. Arreglo: leer el borrador.
- **Bandera de isla chica inerte con malla (bloque 6c):** en trunks ruteados no cambia nada (el router siempre deja juntas de ruta). Decidir si debe afectar al trunk ruteado o eliminarla.
- **Tope del puente de cavidad que nunca rechaza (bloque 6c):** mide desde el contacto `upper`, que el builder ordena al tip, así que el tramo da ~0. Arreglo: medir entre los dos contactos.
- **Escaneo de islas no reproducible (bug DF, bloque 8a):** el escáner nativo devuelve los islotes vóxel en orden de `HashMap` (aleatorio por proceso), así que los ids `v{n}` cambian entre escaneos del mismo modelo. `build_perimeter_loops` recorre los bordes en ese mismo orden, y cambia de una corrida a otra cómo se parten los bucles, su inicio y su sentido, y con eso el inset (hasta 0,22 mm en Z). SC ya fija ambos órdenes: islotes por id de creación y bordes por clave. Queda por ver si conviene otra regla para elegir el inicio y el sentido del bucle.
- **`clipper-lib` lanza en una rama rara (bug DF, bloque 9a):** `ProcessEdgesAtTopOfScanbeam` usa `ePrev2`, que no existe (en C# es `ePrev`), así que esa rama lanza `ReferenceError`; y `ClipperLib.Error` hace `alert()`, que en el WebView abre un diálogo y sigue. SC devuelve error en ambos casos; arreglarlo es usar `ePrev`, como C#, y no seguir tras un error.
- **Islas del raft repetidas (bug DF, bloque 9c):** `polygonsFromPolyTree` recorre con `GetFirst`/`GetNext` (que ya visita a todos los descendientes) y además baja por los hijos de cada nodo. Una isla dentro de un agujero sale varias veces, y la base chaflanada dibuja una copia por cada una: caras superpuestas en la malla exportada (3 de los 167 rafts del oráculo de 9c). Arreglo: recorrer solo las raíces (`Childs()` del árbol) y dejar que la recursión baje.
- **Caché de sombra mundial con clave incompleta (bug DF, bloque 9c):** la clave de `worldShadowCache` (modelo inclinado) omite la posición X/Y. Dos modelos inclinados que compartan geometría, o uno que solo se movió en XY, reciben la holgura del primero en el lugar equivocado. Arreglo: incluir X/Y en la clave, o cachear la sombra en el marco local y transformarla. SC calcula de cero.
- **Excepción de Clipper aborta todo el export (bloque 9c):** nada la captura en el paso del raft. Arreglo: degradar a raft sin holgura (o sin raft para ese modelo) y avisar.
- **Raft recalculado de más (rendimiento, bloque 9c):** cada modelo recalcula la huella de todos los modelos en la placa (`collectModelPlateFootprint`), aunque la banda sea la misma, y el modo línea vuelve a inflar la holgura que la huella ya infló. Medir antes de optimizar.
- **El slice ignora los ajustes de raft por modelo (bug DF, bloque 10):** `buildSupportAndRaftWorldTriangles` usa `getRaftSettings()` global, mientras el export (`ExportManager`) usa `getRaftSettingsForModel`. Un modelo con raft propio se exporta con uno y se slicea con otro. Arreglo: resolver los ajustes por modelo también en el slice.
- **Raft no manifold (DF, SC-81):** la pared de DF se enreda en footprints cóncavos y la base recortada queda con uniones en T (earcut descarta puntos colineales de la tapa). El rasterizador de DF lo tolera; un slicer de contornos no. Ya arreglado detrás de `robust_raft_geometry` (encendido en SC): falta decidir si la pared vertical de Clipper debe recuperar el chaflán de DF arriba, y medir el costo de `split_t_junctions` (O(aristas abiertas × vértices)) en rafts grandes.
- **Costos evitables (rendimiento):** la consolidación clona el borrador entero por cada host candidato (`pruned`), y `buildGroupPairs` es O(n²) con vecinos recalculados. Medir antes de optimizar.

**Cierre:** Cada ítem aplicado con su test propio y la paridad DF intacta con la opción apagada, o descartado con razón escrita; la lista se vacía o queda justificada.

**Depende de:** porteo DF completo (rama `sc/df-supports`); SC-75 es el primer ítem. Prioridad: normal. Estado snapshot: backlog.

## SC-77

**Investigar mover a GPU el pipeline de soportes** — Ideas / rendimiento

Idea de Damian al portar la detección de islas: DF voxeliza en GPU, y la detección nativa puede no coincidir bit a bit en esa etapa. Evaluar qué partes del pipeline (voxelización, campo de distancias, raycasts, router, cobertura) ganan en GPU y con qué tecnología (wgpu/compute shaders u otra opción abierta y mantenida). Considerar el costo en determinismo y reproducibilidad, la portabilidad en Windows y los drivers, y el respaldo en CPU.

**Cierre:** Informe con perfil medido del pipeline nativo, candidatos con ganancia estimada, opción tecnológica con licencia revisada y un prototipo de una etapa con su comparación contra CPU; sin adoptar nada sin medición ni respaldo en CPU.

**Depende de:** porteo DF completo, sus mejoras (SC-76) y los benchmarks (SC-82). Prioridad: low. Estado snapshot: triage.

## SC-78

**Fachada del core de soportes DF portados** — Línea de soportes DF / soportes

Una sola entrada sobre `support-gen-df`, del estilo `generate_supports(escena, ajustes) -> soportes + raft`: escaneo de islas, derivación, plan de auto-soportes con bracing y rafts de exportación. Cancelable y con progreso por etapa, errores tipados (incluido `ClipperError`) y sin estado global. Reutiliza el contrato de jobs de SC si ya existe (SC-37); si no, define el mínimo y lo deja compatible.

**Cierre:** Prueba que corre la fachada sobre escenas del corpus y da exactamente lo mismo que las llamadas por módulo de los tests de paridad; cancelación probada a mitad de cada etapa sin resultados parciales instalados.

**Hecho (2026-10-25):** `libs/sc-geom/support-gen-df/src/pipeline.rs`, con `generate_supports(models, state, settings, new_id, progress, cancel)` y `build_rafts`. Por modelo: geometría mundial como `prepareWorldGeom`, escaneo nativo y de overhangs con los valores por defecto del panel de islas, derivación y plan con bracing sobre el estado que dejaron los modelos anteriores. Al final, los rafts del export con la holgura de los modelos visibles. `tests/pipeline.rs` arma dos escenas con mallas del bloque 8a (cubos y mesa con raft sólido; shell rotado y knot oculto con raft de línea por modelo) y verifica:
- igualdad exacta con las etapas llamadas una por una (islas, resultados, estado de soportes y mallas de raft);
- el orden de los reportes de progreso;
- `Cancelled` en cada uno de los 9 límites de etapa.

Límite: la cancelación se revisa **entre** etapas; una etapa larga (el plan) no se interrumpe por dentro. Eso queda para el contrato de jobs (SC-37) cuando SC-79 lo pida.

**Depende de:** porteo DF completo (hecho). Prioridad: high. Estado snapshot: done.

## SC-79

**Rebanada vertical GUI: auto-soportes y raft DF de punta a punta** — Línea de soportes DF / soportes

Cargar modelo → auto-soportes → raft → preview en el viewport → slice → exportar, usando la fachada SC-78 desde un job de la GUI. Un undo por operación y resultados obsoletos descartados al editar la escena. Sin interacciones finas (arrastre de joints o knots, snapping, multiselección: van con SC-12).

**Cierre:** Damian genera soportes y raft sobre modelos reales desde la GUI, los ve en el viewport, deshace y exporta; compilación y chequeos focalizados registrados.

**Avance (2026-10-25), pendiente de la prueba de Damian:**
- Hizo falta el teselado de soportes (bloque 10 de `DF-PORT.md`): los datos de DF no son malla.
- El adaptador `libs/sc-geom/support-gen-df-sc` hace dos cosas:
  - corre la fachada sobre la escena SC: cada hoja de malla es un modelo en mundo, se rechaza CSG y se salta la malla de soportes de una corrida anterior;
  - devuelve los soportes y el raft como una malla soldada, `Supports (DragonFruit)`.
- La GUI (`apps/slicer-gui/src/df_support_run.rs`) agrega el menú **Tools → Generate Supports (DragonFruit)…** con estas reglas:
  - corre como job con progreso por etapa y botón Cancel en el panel;
  - si la escena cambió durante la corrida, descarta el resultado;
  - si no, pega la malla como **un** objeto deshacible y muestra un resumen.
- **Primera prueba de Damian (2026-10-25):** cubo rotado; soportes, brazos y raft almenado bien. La placa mostraba el aviso de rayas rojas porque 55 vértices de las bases quedaban en −1,7e-16 mm (redondeo al rotar los cilindros). El adaptador ahora apoya en Z = 0 todo vértice a menos de 1e-4 mm por debajo; la salida del port sigue exacta.
- **Tubos suaves con biconos (pedido de Damian, 2026-10-25):** las esferas de las juntas se veían como escalones. `features/slicing/smooth_shafts.rs` (de SC, no de DF) arma cada cadena de tramos como un tubo continuo:
  - en los codos dobla sobre un anillo ingletado en el plano bisector;
  - donde DF pone una esfera, ensancha en un bicono: cintura 1,3× el radio y collares cónicos de 2,5× la cintura, como mucho el 40 % del tramo.
  - Segunda vuelta tras la prueba de Damian:
    - el cono de contacto de cada tramo es ahora el final del tubo (codo ingletado y luego estrechamiento hasta la punta), sin la junta abierta que dejaba la esfera faltante;
    - solo hay bicono donde el tramo dobla más de 8°, no en juntas rectas.
  - Va detrás de `ShaftStyle`: `DragonFruit` (paridad, el valor por defecto del port) o `Smooth`, que es el que usa la GUI.
  - Tests: tubos cerrados y orientados hacia afuera, bicono solo en las juntas, y sobre los 52 estados de 8c menos triángulos que DF sin abrir la malla. La paridad DF sigue exacta.
- **Chequeos:** el adaptador sobre un cubo elevado (soportes, raft en la placa, exclusión de la salida anterior, cancelación) y un test de cableado de la GUI.
- **Límites de este corte:**
  - la altura de capa del escaneo es 0,05 mm fija;
  - los ajustes son los de DF por defecto;
  - una segunda corrida agrega otro objeto en vez de reemplazar el anterior;
  - las uniones donde una rama sale de un tronco siguen siendo dos tubos que se cruzan (sin unir);
  - el slice de esa malla (soportes superpuestos en una sola malla) es SC-81.

**Cerrado (2026-10-25):** Damian generó soportes y raft sobre modelos reales (cubo rotado, ballena) desde la GUI, los vio en el viewport, deshizo y rehízo con Ctrl+Z/Ctrl+Y y exportó el STL. La revisión del STL exportado en Caliper muestra 1387 componentes, 0 aristas abiertas, 1940 aristas no-manifold y 2002 caras duplicadas: la malla de soportes suelda piezas que se tocan y repite piezas coincidentes. Que eso slicee bien es SC-81.

**Depende de:** SC-78. Prioridad: high. Estado snapshot: done.

## SC-80

**Fallo de stitching de `sl1cer` ("dangling endpoints")** — Línea de soportes DF / slicing

Hallado en la fase 0 del porteo (`docs/sc/DF-PORT.md`): Wolverine centrado falla en el stitching de `sl1cer` a Z 32.5 mm con "2 dangling endpoints". Es de la infraestructura SC, no de los soportes. Reproducir con el caso mínimo, encontrar la causa y arreglarla sin debilitar las protecciones de geometría inválida.

**Cierre:** Caso de regresión que falla antes y pasa después; Wolverine slicea completo; sin regresiones en los chequeos de slicing existentes (SC-33).

**Resultado (2026-10-25):** la causa no estaba en el stitcher sino en los segmentos que recibe. Cuando un triángulo queda entero en el plano (coplanar) y su vecino por una arista sube por encima, la regla de aristas en el plano (`resolve_on_edge_contour_segment`, `libs/sl1cer/src/mesh_slice.rs`) sólo emitía segmento si los dos terceros vértices quedaban estrictamente a lados opuestos; con el tercer vértice del vecino en el plano no emitía nada y los dos vértices de esa arista quedaban con 1 extremo cada uno ("2 dangling endpoints"). En Wolverine, a Z 32.5 mm: cara coplanar 898401, vecino 898395 por encima. Arreglo: el triángulo coplanar cuenta como apenas debajo del plano (desempate simbólico, como si el plano subiera un infinitésimo) y el segmento lo emite la cara no coplanar. Las protecciones no cambian: el stitcher sigue rechazando cualquier vértice o arista con extremos impares. Convención elegida: una cara plana justo en el plano cuenta como sólida por debajo (sólido en [zmin, zmax)); invertirla es cambiar `Below` por `Above` en ese desempate.

**Evidencia:** fixture `coplanar_face_crossing_solid` (`libs/mesh-fixtures`) y prueba `coplanar_face_crossing_solid_at_z_zero_stitches_through_the_coplanar_border` (`libs/sl1cer/src/tests_mesh_slice.rs`): sin el arreglo falla con `dangling_endpoints: 2`, con el arreglo da 1 anillo de área 1. Wolverine centrado (SHA256 `5e4e7573…21eeb77489`, privado en `.local/`): barrido en los 11 505 Z distintos de sus vértices, antes fallaban 233 planos y ahora 0 (28 s, release). `sc-slicer-cli slice --no-auto-place` (Saturn 3 Ultra, capa 0.05, primera capa 0.075, con un plano en cada múltiplo de 0.05, incluido 32.5) antes cortaba en Z 13.8 mm y ahora termina las 700 capas en 6.6 s; con primera capa 0.05 tarda 6.8 s. `cargo test -p sc-sl1cer -p sc-mesh-fixtures -p sc-print-glue -p sc-sla-formats -p sc-project-formats -p sc-slicer-cli` en verde salvo `bundled_profiles` (2 pruebas de round-trip de perfiles que fallan por el CRLF del checkout Windows, ajenas al cambio); clippy sin avisos.

**Depende de:** sin dependencia técnica previa. Prioridad: high. Estado snapshot: done.

## SC-81

**Soportes y raft DF en el slice y el archivo de salida** — Línea de soportes DF / slicing

Llevar las mallas de soportes y raft de la fachada al slice y a los writers: que el raft quede en las primeras capas, sin huecos ni solapes, y que lo sliceado coincida con el preview.

**Cierre:** Escenas del corpus slicedas con soportes y raft, capas inspeccionadas contra la geometría, archivo de salida validado con el writer correspondiente (SC-35); impresión física solo con autorización (SC-66).

**Avance (2026-10-26): malla de soportes cerrada y manifold.** `sl1cer` pide hojas edge-manifold. La malla que pega la GUI ya lo es en las 13 escenas de 8a (`support_meshes_of_real_scenes_are_edge_manifold`):
- el teselado marca dónde termina cada pieza (`TriangleSink`) y el adaptador suelda por pieza (tolerancia 1e-5 mm), sin soldar piezas que se tocan y descartando piezas repetidas;
- los tubos suaves se parten en codos de más de 120° (tubo, esfera, tubo), donde el anillo de inglete se cruzaba;
- con `robust_raft_geometry` (por defecto en SC, apagado para paridad), la pared del raft se arma con Clipper y la base recortada se arma sobre anillos sin puntos colineales, partiendo los triángulos de la tapa que earcut dejó con uniones en T (`robust_wall.rs`).

**Avance (2026-10-26): slice de punta a punta.**
- `sl1cer` agrupa contornos exteriores y agujeros por shell (componente conexo) dentro de cada hoja: shells superpuestos ya no se toman por agujeros (`shells.rs`; tests con cubos anidados y cruzados, que fallan sin el arreglo).
- Penetración de puntas portada para los ajustes AA por defecto de DF (`tip_offset.rs`, 180 casos exactos); la GUI y la CLI la calculan con el pitch y la capa de la impresora y resina en uso (0,25 mm en la Saturn 3 Ultra a 0,05 mm).
- CLI: `slice --df-supports [--df-lift MM]` genera soportes y raft antes de slicear.
- Asmodeus (Saturn 3 Ultra, 0,05 mm, resina Elegoo 8K ABS-like): 253 raíces, 1436 capas, `.ctb` de 36 MB. En PNG por capa, base sólida del raft en las capas 0–9, pared almenada y conos en la 12, troncos hasta la base del modelo y ninguna capa vacía.

**Prueba de Damian (2026-10-26, GUI → `.ctb` de Elegoo → UVTools):** el archivo abre bien y el slice se ve correcto; UVTools marca dos tramos de "high peel force", que dependen del modelo. Sobraban 306 capas vacías al final (contenido hasta 92,5 mm, archivo hasta 123,1 mm). El schedule de capas tomaba la altura de `Scene::aabb()`, que para una malla rotada es la caja local transformada (Arvo), una cota holgada. Ahora `slice_profile_for_scene` usa `Scene::tight_aabb()`, los vértices ya ubicados con la misma cuenta que el bake; hay tests en scene y print-glue, y el de print-glue falla sin el arreglo. La Anycubic M7 Pro no exporta (`azf` sin writer): es SC-19/SC-36.

**Cerrado (2026-10-26):** Damian volvió a slicear el modelo 617 con soportes DF desde la GUI (Elegoo, 0,1 mm) y lo revisó en UVTools: el archivo abre bien, tiene 937 capas y 93,7 mm de altura total, y no queda ninguna capa vacía. La impresión física sigue en SC-66; la validación sistemática de writers, en SC-35.

**Depende de:** SC-79, SC-80. Prioridad: high. Estado snapshot: done.

## SC-82

**Benchmarks del pipeline de soportes en escenas reales** — Línea de soportes DF / rendimiento

Medir por etapa (escaneo de islas, derivación, plan, bracing, raft, slice) sobre pocas escenas representativas, frío y caliente, con memoria. Cada medición responde una pregunta concreta; los resultados deciden qué optimizar en CPU y qué evaluar para GPU (SC-77). Parte del marco general de SC-54.

**Cierre:** Corpus y hardware registrados, perfil por etapa con los cuellos de botella identificados y una lista priorizada de optimizaciones con la ganancia esperada.

**Primera medición (2026-10-25, ballena de Damian, timings de `perf.rs`; log en `%LOCALAPPDATA%\soulcrafted\slicer\data\logs\df-supports-timings.log`):** 9,0 s en total. Escaneo de islas 2,7 s, escaneo de voladizos de la sopa 2,9 s, plan 2,8 s (colocación 2,2 s, de la que el router V3 son 1,6 s en 365 llamadas) y el resto ~0,6 s.

Primer ajuste sin cambio de resultados:
- el escaneo combinado ya no clasifica voladizos, cuyo resultado nadie leía (`scan_islands_without_overhangs`; test de igualdad sobre las 13 mallas de 8a);
- los dos escaneos corren en paralelo, así que el tiempo pasa a ser el del más lento.

Siguientes candidatos: el router (por llamada, ~4 ms) y la clasificación de voladizos de la sopa.

**Segunda medición (2026-10-26).** Banco reproducible: `tests/bench_real_model.rs`, ignorado; recibe `SC_BENCH_STL`, `SC_BENCH_HEIGHT`, `SC_BENCH_ROT` y `SC_BENCH_RUNS`. Imprime el reporte y un digest del estado de soportes, que tiene que quedar igual tras cada optimización. Escena: el modelo 617 de Damian (1,9 M triángulos) escalado a 90 mm, con 849 raíces. Pasó de 25,8 s a 22,5 s con el mismo digest:
- **Voladizos, de 6,4 s a 2,0 s.** El mapa arista → triángulos era un `HashMap` por hilo con merge (1,9 s a escala real); ahora es un orden paralelo de pares en CSR (`EdgeTris`, 0,12 s). Además, el escaneo reusa la malla soldada del pipeline (`overhang_and_stability`, mismo épsilon), y la soldadura ya no se hace dos veces.
- **Mapa de columnas del router, en paralelo.** `ColumnClearanceMap::build` llena franjas de filas en paralelo con los vértices agrupados por fila. El mínimo por celda no depende del orden, así que el resultado es idéntico.

Dónde está el tiempo ahora (617 a 90 mm):
- **Router, 8,3 s en 1950 llamadas:** la búsqueda de juntas (`find_escape_joint`) da 5,8 M pasos diagonales cortos. El costo real son 1,16 M de celdas SDF nuevas, cada una una consulta `closest_point_to_point` al BVH (~4,5 µs). Las columnas ya son O(1) por el mapa.
- **Escaneo de islas, 4,4 s.** Ahora define el tiempo de los escaneos.
- **Resto de la colocación, ~6 s; consolidación, 1,5 s.**

Opciones para el router, de menor a mayor alcance:
1. acelerar la consulta al BVH sin cambiar qué triángulo gana en empates (el signo sale de su normal);
2. precargar en paralelo las celdas que va a pedir una caminata;
3. una caché concurrente y caminar las direcciones en paralelo.
Las tres necesitan probar que el digest no cambia.

**Consulta al BVH (2026-10-26).** Cada consulta recorre unos 61 nodos y prueba unos 70 triángulos; en total son ~2 M consultas por corrida y 6,7 s.
- **Se queda:** un triángulo solo gana si está *estrictamente* más cerca que el mejor, y nunca está más cerca que su caja. Si su caja ya queda más lejos que el mejor, con un margen de 1e-9 mm² (mil veces el error de redondeo), no se lo prueba. Esto bajó las consultas a 5,7 s y el router de 8,3 a ~7,4 s, con el mismo digest en Asmodeus y en el 617.
- **Medido y descartado, sin ganancia:**
  - vértices contiguos por triángulo en orden BVH (el costo no es de memoria);
  - cajas de nodos en `f64`;
  - arrancar la búsqueda con una cota Lipschitz sacada de las celdas vecinas en caché. Con 6 vecinas ahorra en la consulta lo que cuesta buscarlas; con 26, empeora.

Lo que queda es estructural: el recorrido en sí (~2,7 µs por consulta) o hacer menos consultas, con las opciones 2 y 3 de arriba.

**Pre-escaneo (2026-10-26):** los escaneos y el BVH salen del tiempo de espera; ver SC-83, primera etapa. Además, los escaneos internos usan `rayon::join` en vez de hilos propios, así que corren en el pool de quien llama, con el mismo resultado.

**Tercera medición (2026-10-26, rama `claude/support-perf-39869c`).** El 617 a 90 mm pasó de **20,6 s a 11,7 s**. Los digests no cambiaron: 617 `a4919dae4f59d8fe`, Asmodeus `dd087dddd80c4a8f`. Las suites de `support-gen-df` y `support-gen-df-sc` pasan y clippy queda limpio. La máquina tiene 8 núcleos y 16 hilos; con otra sesión compilando a la vez, los tiempos suben mucho (ver abajo).

| Etapa | Antes | Después |
|---|---|---|
| Búsqueda de juntas (router) | 5,8 s | 3,4 s |
| Escaneo de islas | 4,4 s | ~1,3 s (voladizos, en paralelo, ~1,2 s) |
| Colocación | 12,2 s | 7,5 s |
| Consolidación | 1,5 s | 0,6 s |

- **Router, prefetch especulativo.** Antes de la búsqueda real, las caminatas de las 12 direcciones corren en paralelo, en tramos de 6 pasos y en orden de preferencia, contra vistas descartables de la caché. Guardan las respuestas al BVH que calcularon. La búsqueda real sigue secuencial y en su orden, y toma de ahí lo que coincide. Una respuesta guardada es exactamente lo que se habría calculado: es función pura del punto local, la cota, el BVH y la matriz. Por eso el resultado es idéntico por construcción y la duda sobre el orden `bounded_distance_at`/`distance_at` no aplica. La march quedó genérica sobre el almacén de celdas (`MarchField`), así que el código es uno solo.
  - Acierto: 99,8 % de las consultas.
  - Desperdicio: ~45 % de consultas de más. El corte por fin de caminata, junta mejor y presupuesto lo bajó de 2,6 M a 2,1 M; la memo compartida entre hilos evita duplicados.
  - El límite es el throughput (16,9 s de CPU en 2,2 s de pared), no el camino crítico.
  - Las tablas usan `ahash` en vez de SipHash.
  - **Sensibilidad:** con 1 hilo la búsqueda tardó 16,6 s, y con la CPU tomada por otra compilación, 22 s. Por eso se especula solo con ≥ 4 hilos en el pool: con 2 hilos queda en 5,7 s, como antes, y con 4 baja a 3,9 s.
- **Islas.**
  - El snap a superficie metía ~30 k triángulos por isla en un `HashSet`; ahora filtra por la caja XY y visita cada triángulo una vez: 1,8 s → 0,1 s. El orden de visita cambió, pero solo decide en empates exactos de distancia, que el `HashSet` ya resolvía al azar.
  - El rasterizado arma las corridas desde los tramos del scanline, sin la máscara densa de 2100×2218 por capa: 1,6 s → 0,5 s. Un test lo compara con el relleno denso.
  - Al cortar, los cruces van en `SmallVec` y el hash es `ahash`.
  - En los mínimos, el BVH de rayos se arma junto a las tablas de vértices y el orden de aristas es paralelo.
- **Colocación y consolidación.**
  - `place_one_candidate` coloca en el borrador mismo (`&mut`) en vez de clonar el estado entero por candidato, y el abanico y el merge devuelven nudo y miembro.
  - La consolidación saca el tronco del borrador y, si se queda, lo reinserta en su índice (`shift_insert`), en vez de clonar por anfitrión.
- **Medido y descartado:**
  - especular caminatas enteras por dirección, que deja el camino crítico en la dirección 0 (64–127 pasos): búsqueda en 5,2 s;
  - tramos de 2, 10 o 20 pasos en vez de 6: sin diferencia que salga del ruido.
- **Lo que queda**, de mayor a menor:
  - router, 4,6 s, del que la búsqueda es 3,4 s; sigue limitada por throughput;
  - colocación fuera del router, ~3 s, donde `collect_fan_shaft_points` por candidato son ~0,5 s y se podría volver incremental;
  - voladizos, ~1,2 s, en paralelo con islas;
  - candidatos, 0,6 s.

Hallazgos de paso, ajenos a esta línea:
- `viewport_click_tests::pointer_down_on_handle_does_not_drag_outside_transform` ya fallaba antes de estos cambios;
- clippy 1.97 marca 6 avisos en `camera.rs`, `export_print_file.rs` y `viewport/controller.rs`.

**Depende de:** SC-79. Prioridad: normal. Estado snapshot: backlog.

## SC-83

**Pestaña Supports con cálculo especulativo y caché por etapas** — Línea de soportes DF / soportes

Idea de Damian (2026-10-25): una pestaña Supports propia (sin herramientas de transformación) que, al abrirse, corre el pipeline en segundo plano y deja el resultado en espera, para que Autosupport sea prácticamente instantáneo.

- **Caché por etapas**, cada una con su clave:
  - escaneo de islas: geometría y pose;
  - plan: además, ajustes de soportes y estado de soportes existente;
  - teselado: además, estilo de malla.
  - Cambiar un ajuste en Supports rehace solo el plan; volver de Transform sin mover nada no recalcula.
- **Salir a Transform cancela el cálculo.** Los soportes **no se borran**: si la pose cambió quedan marcados como desactualizados. Los automáticos se regeneran solos al volver y las ediciones manuales piden confirmación, siempre con undo (protección de datos del usuario).
- **Prioridad baja** del trabajo en segundo plano, con opción para apagarlo.
- **Primera medición** (ballena, 2026-10-25, timings de `perf.rs`): ~2,5 s en total, de los cuales la colocación del plan son ~2,1 s. Qué cachear primero lo deciden esos números (SC-82).

**Primera etapa (2026-10-26, sin pestaña nueva): pre-escaneo en segundo plano.**
- **Core:** `prescan_models` corre lo que depende solo de la geometría (escaneo de islas, de voladizos y BVH) y lo guarda en un `ScanCache`. La clave es el hash de la geometría ya ubicada en mundo más los ajustes de escaneo, así que una pose o un ajuste distintos no coinciden y la corrida escanea como siempre. `generate_supports_with` la reusa; el test confirma que la corrida es idéntica con y sin caché, y que un modelo movido no la usa.
- **Adaptador:** `start_df_prescan` hornea y escanea en un pool de la mitad de los núcleos, y reusa los modelos de un pre-escaneo anterior que todavía coinciden. `start_df_supports` también hornea en el worker, así que el botón ya no traba la UI.
- **GUI:** un evento de escena solo guarda la hora del último cambio; es lo único que agrega al camino del transform. Un poll cada 150 ms arranca el pre-escaneo cuando la escena lleva 500 ms quieta, lo cancela si la escena cambia y le pasa el resultado a Generate Supports. Se apaga en Preferencias → Slicing.
- **Medición (617 a 90 mm):** el pre-escaneo tarda 5,1 s en segundo plano; Generate Supports pasa de ~20–22 s a 15,2 s, con el mismo digest.

**Prueba de Damian (2026-10-26):** el pre-escaneo no se usó. Apretó Generate Supports mientras todavía corría; el botón lo canceló y el escaneo cancelado siguió compitiendo por CPU hasta terminar su modelo. Decisión de Damian: no depender de "que el usuario espere X segundos". El cálculo lo dispara la pestaña Supports al entrar. El pre-escaneo queda **apagado por defecto** (opción en Preferencias → Slicing), con la caché por geometría lista para esa pestaña.

**Cierre:** Damian entra a Supports, aprieta Autosupport sin espera perceptible en sus modelos habituales; cambiar un ajuste no reescanea; ir a Transform y volver sin mover nada no recalcula; mover el modelo marca los soportes y los regenera o pregunta según sean automáticos o editados.

**Depende de:** SC-79 (soportes en la GUI) y SC-37 (contrato de jobs y resultados obsoletos). Entra con el retrabajo de la UI. Prioridad: normal. Estado snapshot: backlog.

## SC-84

**Rediseño de la UI: Transform → Hollow / Supports → Slice → Export** — Editor / UI

Pedido de Damian (2026-10-26): antes de seguir optimizando, una UI que valga la pena.
- **Recorrido:** Transform → Hollow / Supports (dos pestañas separadas que se usan en cualquier orden) → Slice → Export.
- **Estilo:** oscuro sobrio, tipo herramienta profesional. Grises y un solo acento ámbar: la paleta "Synrg Neutral" de su DragonFruit, sin gradientes ni efectos web.
- **Navegación:** pestañas arriba.
- **Base:** su propio `docs/internal/ui-ux-roadmap.md` de DragonFruit (riel de herramientas monocromo, inspector fijo a la derecha, W/E/R, barra de estado; referencia Caliper).

**Mockup interactivo:** https://claude.ai/artifact/EkuMQE5HaSJ3Gzfp2Y8Z9v (privado).

**Aprobado como base (Damian, 2026-10-26)**, con dos ajustes ya aplicados al mockup:
- atajos G mover, R rotar y S escalar, como en la app actual;
- el inspector abre la tarjeta de la herramienta activa y pliega las demás; con Select (Q) se ven todas.

Damian asume que un slicer nunca queda del todo "streamlined"; esto es la base para iterar.

**Disparo del pre-cálculo de soportes (Damian, 2026-10-26):** se dispara al entrar a Supports **o a Hollow**. En las dos pestañas la pose ya está decidida, y con hollow puede hacer falta soportar dentro de la cavidad. Usa la caché por geometría de SC-82/83, ya lista.

**Relevamiento (2026-10-26):**
- **SC hoy:**
  - secciones Prepare/Transform/Slice con reglas en `workspace_section.rs`;
  - `MainWindow` plano de ~1400 líneas y ~100 propiedades, menús duplicados, paneles flotantes copiados tres veces;
  - sin paleta propia ni íconos, estilo fluent por defecto;
  - el render wgpu se compone como `Image`, así que los overlays de Slint van encima.
- **Hollow de DF (base a portar en SC-39…46):**
  - vóxeles con distancia chamfer 26-vecinos, cavidad suavizada (marching tetrahedra + Taubin) y estabilización manifold;
  - infill lattice/pillars, áreas sólidas con lazo, drenajes con profundidad automática;
  - cache con grilla local al modelo (mover y rotar no recalculan) y preview rápida separada del Apply.
- **A corregir de DF en el hollow:**
  - el tamaño de vóxel efectivo se recorta en silencio (grilla 24–192) sin mostrarse;
  - no hay progreso ni cancelación;
  - no hay tooltips ni resultado (ml ahorrados).

**Primera entrega en Slint (2026-10-26):**
- **Kit propio** en `ui/kit/`: paleta `Theme` (oscura de referencia, clara siguiendo el esquema), íconos de trazo, `ScButton`, `WorkflowTab`, `ToolButton`, `Card` plegable, `InfoRow`, `StatTile`, `Note`.
- **Ventana:** barra superior con nombre de archivo y pestañas, riel de herramientas por pestaña, viewport, inspector fijo (`ui/workspace/inspectors.slint`) y barra de estado. Se fueron los paneles flotantes (objetos, slice, vista de capa), la píldora de secciones y la sección Prepare.
- **Secciones** `Transform | Hollow | Supports | Slice | Export`:
  - gizmo y herramientas de layout solo en Transform; undo/redo hasta Supports; repair en Transform;
  - si la malla tiene problemas, Transform abre con la tarjeta de repair adelante, y se apaga sola tras reparar.
  - El test `pointer_down_on_handle_does_not_drag_outside_transform`, que ya fallaba, pasa ahora: Prepare permitía el gizmo.
- **Supports:**
  - al entrar a Hollow o Supports arranca el pre-escaneo;
  - Generate con un pre-escaneo en curso de la misma escena lo espera en vez de cancelarlo;
  - el inspector muestra progreso, Cancel y el resumen de la última corrida.
- **Slice:** impresora, resina, primera capa y botón; el resultado muestra capas y la imagen de la capa en el inspector; el slider queda en el viewport.
- **Export:** formato, nota, exportar para la impresora, guardar proyecto y exportar malla.

**Pendiente:**
- campos numéricos de transformación y herramientas por modo (hoy Move/Rotate/Scale del riel arman el foco de teclado);
- panel de ajustes de soportes;
- motor de hollow (SC-18);
- estimaciones y chequeos en Slice;
- traducir los textos nuevos;
- limpiar del lado Rust las propiedades de geometría de los paneles flotantes, que quedan sin uso;
- prueba de Damian.

**Cierre:** Damian aprueba el mockup. Después la app tiene:
- las pestañas del recorrido, con reglas por etapa;
- la paleta y los componentes propios (campo numérico con unidad, segmentados, tarjetas, riel, barra de estado);
- el inspector fijo por pestaña;
- la pestaña Supports conectada a SC-83 (cálculo al entrar, ajustes visibles);
- la pestaña Hollow con su panel listo para el motor de SC-18;
- y Damian prueba los gestos y el flujo completo.

**Depende de:** SC-83 (Supports) y SC-18 (motor hollow) para el contenido real de esas pestañas. Prioridad: high. Estado snapshot: in progress.

## SC-85

**Ayudas visuales del gizmo: grados, distancias y línea del eje** — Editor / UI

Pedido de Damian (2026-10-26):
- mientras se rota, mostrar cuántos grados se lleva; mientras se mueve, cuánto se movió;
- con G, R o S y un eje fijado (X/Y/Z), una línea del color del eje que atraviesa el origen del objeto, como en Blender (Z = línea azul vertical).

Va junto con los campos numéricos de SC-26.

**Primera entrega (2026-10-26):**
- **Chip arriba del viewport,** en cualquier gesto (gizmo o G/R/S modal), con lo que lleva hecho: al mover, el delta por eje o el total en mm; al rotar, los grados (con signo si hay eje fijado); al escalar, el %.
- **Línea del eje** en G/R/S con eje fijado, del color del eje y a través del objeto, dibujada como `Path` de Slint con el pivote en pantalla y los ejes de cámara del gesto.
- **Cálculo** una vez por frame junto con la lectura numérica (`gesture_delta_text`, con test).

Falta la línea del eje en los arrastres del gizmo (el gizmo ya muestra su eje) y la prueba de Damian.

**Cierre:** las tres ayudas visibles en gestos de gizmo y modales, legibles sobre cualquier fondo, probadas por Damian.

**Depende de:** SC-84, SC-26. Prioridad: high. Estado snapshot: backlog.

## SC-86

**Panel completo de soportes, lift y presets de Damian** — Soportes / UI

La pestaña Supports necesita todas las opciones de `PipelineSettings` (contactos, puntas, troncos, ramas, bracing, raft, densidad, escaneo) y el **lift del objeto** (hoy solo existe en la CLI, `--df-lift`), agrupadas como el mockup.

Presets:
- **Primeros presets:** los packs que Damian usa y modificó, "MIni - 35mm - Tall" y "Figure - 75mm +", sobre sus geometrías J3D (contactos de 0,18, 0,24, 0,30 y 0,40 mm; punta Ø 1,1 × 2,3 mm; pie Ø 5 × 0,25 mm). Fuente: SC-07, `docs/sc/evidence/lychee-support-review-20261022.md`; datos crudos en `.local/research/lychee-static-20261022.zip`.
- Las equivalencias con los parámetros de DF se documentan campo por campo, y sin intuición donde el informe avisa trampas (`densityValue` es separación, `tip.angle` no es inclinación).
- Hay que guardar, importar y exportar presets.

Cambiar un ajuste re-planifica sin re-escanear (caché por etapas, SC-83).

**Primera entrega (2026-10-26):** panel en la pestaña Supports.
- **`DfSupportOptions` (adaptador):** los mismos controles que expone el panel automático de DF:
  - tamaño Detail/Structure/Anchor, área por soporte, ángulo de voladizo, escala de tamaño, isla mínima y cobertura;
  - estabilización, bracing y forma Smooth/Classic;
  - raft sólido, de líneas o apagado, con espesor y pared.
  Cada valor se acota a los límites de DF; los valores por defecto dejan `PipelineSettings` idéntico.
- **Lift del objeto** editable desde Supports (y Drop to plate).
- **Presets:** tres de fábrica (los niveles de DF) más los del usuario, guardados en TOML en la carpeta de datos (guardar y borrar). Las opciones se recuerdan entre sesiones y viajan en el pre-escaneo, así que la clave de escaneo coincide.

Falta:
- los presets de Damian (Mini 35mm Tall / Figure 75mm+), que necesitan el mapeo campo por campo Lychee → DF documentado en SC-07, sin inventar equivalencias;
- guardar las opciones dentro del proyecto;
- prueba de Damian.

**Cierre:** cada opción del pipeline es editable y persiste con el proyecto; lift aplicado; los dos presets de Damian disponibles; Damian genera soportes con ellos en sus minis y los aprueba.

**Depende de:** SC-84, SC-83. Prioridad: high. Estado snapshot: backlog.

## SC-87

**Repair: mejorar con DF y Caliper como extra** — Geometría / reparación

Caliper (`C:/Users/dnaon/Downloads/CaliperDevelop`) es la herramienta de análisis y reparación de mallas de Damian (SynrgStudio): C++ con aritmética exacta, cavidades certificadas y repair validado sobre sus modelos (0.8.5). Su código es cerrado y la fuente privada es suya. Damian quiere aprovecharlo en el slicer, en vez del repair actual o del de DragonFruit (`rust/dragonfruit-mesh-repair`), que considera correcto pero no brillante.

- **Licencia, a resolver primero:** SC es AGPL-3.0 e incluye código de terceros (MyMiniFactory, CTRLback) que Damian no puede relicenciar, así que **no se puede enlazar** un motor cerrado dentro del binario de SC para distribuirlo.
- **Ruta compatible:** Caliper como **programa aparte** (CLI o servicio local) que SC invoca con un archivo y del que lee el resultado. Eso exige que Caliper tenga o gane un modo sin interfaz.
- **Alternativa:** que Damian publique bajo una licencia compatible la parte que quiera integrar. La decisión es suya.
- **Comparación primero:** Caliper, el repair actual de SC y el de DF sobre el corpus (SC-24) y la ballena.

**Decisión de Damian (2026-10-26):**
- **Repair principal:** el de SC, mejorado con lo que sirva del de DragonFruit (`rust/dragonfruit-mesh-repair`). Se compara sobre el corpus antes de portar nada.
- **Caliper como extra:** es gratis y Damian no tiene problema en distribuirlo. Va como **programa aparte**, nunca enlazado dentro de SC, para no mezclar licencias. Gratis no es lo mismo que código abierto: enlazado dentro del binario AGPL exigiría publicar su fuente.
- **Integración:** un botón en Transform, "Reparar con Caliper", que abre la malla en Caliper si está instalado, y si no, enlaza la descarga (`https://github.com/SynrgStudio/caliperreleases/releases/latest`). Más adelante, si Caliper gana un modo por línea de comandos, el resultado puede volver solo a la escena.

**Cierre:**
- repair de SC comparado con el de DF y mejorado donde gane, con medición sobre el corpus y la ballena;
- botón de Caliper funcionando (abrir si está instalado, link si no).

**Depende de:** SC-29. Prioridad: normal. Estado snapshot: triage.

## SC-88

**Vista de capa con zoom y paneo** — Slicing / UI

Pedido de Damian (2026-10-26): la imagen de la capa en Slice le encanta; quiere poder hacer zoom y moverse por ella. La vista se puede ampliar a un panel grande o a pantalla dividida, como el modo Printing de DragonFruit, con 3D a la izquierda y capa a la derecha. Debe mostrar la capa en píxeles reales de la impresora, con antialiasing incluido.

**Primera entrega (2026-10-26):**
- **Vista de capa:** el botón de capas del riel de Slice (L) parte el viewport, con 3D a la izquierda y capa a la derecha (`ui/workspace/layer-view.slint`). Zoom con Ctrl+rueda o con −/Ajustar/+ (hasta 32×, píxeles nítidos desde 4×) y paneo arrastrando o con la rueda.
- **Estimación en la tarjeta RESULT de Slice:** tiempo de impresión, resina y peso, las mismas cifras que lleva el archivo (`ExportJob::program`). Si la configuración no resuelve, muestra el motivo.
- **Pendiente:**
  - la imagen es la del corte que ya existía; falta la capa a resolución real de la impresora con AA;
  - el costo necesita un precio por resina en el perfil;
  - prueba de Damian.

**Cierre:** zoom con rueda y paneo con arrastre sobre la capa, a resolución real, sin trabar el slider; probado por Damian.

**Depende de:** SC-84, SC-34. Prioridad: normal. Estado snapshot: backlog.
