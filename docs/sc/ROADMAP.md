# Roadmap propio de SC

Snapshot de planificación 2026-10-22, posterior al inventario SC-09. **76 tarjetas**; el estado operativo está en [Workboard](https://oc.synrgs.tech/workboard/sc).

No es una promesa de implementar todo. Las ideas son exploratorias, y las tarjetas de hitos agrupan trabajo sin duplicar sus entregables. SC-08/09/14 y SC-24 están completadas en sus alcances; las demás se organizan y no se iniciaron por cerrar el corpus. No hay despacho automático.

## Orden inmediato recomendado

1. SC-24: corpus representativo **completado** (25 casos / 28 assets).
2. SC-14: flujo guardar/abrir/historial **completado**, con corrección de protección dirty.
3. SC-10: especificar primer hito de soportes manuales.

Después: SC-37/38 → SC-11 → SC-12. Hollow: SC-39/40 antes del motor. La validación de perfiles/writers puede avanzar como línea independiente.

## Cómo mantenerlo

- Leer estado actual del board antes de editar; conservar IDs y evidencia histórica.
- Cerrar una tarea sólo con aceptación demostrada y commit/build/artefacto asociado. Inventario estático no significa runtime probado.
- A cada entrega, regenerar INVENTORY.json y ajustar la matriz y este snapshot si cambia el alcance. Guardar snapshot operativo con IDs si se modifica el board.
- Usar dependencias como orden de planificación; no promueven ni lanzan tareas automáticamente.
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
| [SC-12](#sc-12) | backlog | soportes | Editor manual de soportes en viewport | SC-11, SC-14 |
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

**Cierre:** Damian completa flujo manual sin clicks ambiguos; un undo por gesto; cancelación exacta; guardas Slice/loading; soportes sobreviven guardar/reabrir.

**Depende de:** SC-11, SC-14. Prioridad: normal. Estado snapshot: backlog.

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

**Depende de:** SC-14. Prioridad: normal. Estado snapshot: backlog.

## SC-27

**Colocación precisa: centrar, apoyar, alinear y límites** — Desarrollo / editor

Reutilizar print-glue placement y mínimo de vértices. Centrar, drop-to-bed, elevación intencional, alineación y avisos fuera de volumen; aclarar política ante escalas invertidas.

**Cierre:** Dimensiones/posición correctas y un undo; no corregir silenciosamente Z intencional; avisos coinciden con exportación.

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
- **Costos evitables (rendimiento):** la consolidación clona el borrador entero por cada host candidato (`pruned`), y `buildGroupPairs` es O(n²) con vecinos recalculados. Medir antes de optimizar.

**Cierre:** Cada ítem aplicado con su test propio y la paridad DF intacta con la opción apagada, o descartado con razón escrita; la lista se vacía o queda justificada.

**Depende de:** porteo DF completo (rama `sc/df-supports`); SC-75 es el primer ítem. Prioridad: normal. Estado snapshot: backlog.
