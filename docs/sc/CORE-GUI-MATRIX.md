# SC-09 — inventario core / GUI / validación

Auditoría estática del código funcional `22526a29`, 2026-10-22. El inventario
automático identifica **26 paquetes: 20 del workspace principal y 6 del anidado**.
Los hashes en [INVENTORY.json](INVENTORY.json) permiten detectar cambios posteriores.
Son hashes de bytes del checkout; una conversión de finales de línea también los cambia.
No se ejecutaron suites nuevas: las pruebas citadas son evidencia histórica de la
build/commit correspondiente. Lectura de código ≠ validación del flujo completo.

## Actualización runtime SC-14

[SC-14](PROJECT-FLOW.md) agrega evidencia posterior al inventario estático:
14 controles nativos de archivos sobre la baseline y 9 sobre la build corregida;
126 tests focales. Save/reopen, copy/paste/delete y GX con undo/redo conservan
geometría; perfiles embebidos se conservan. Reparación nativa tiene undo exacto.
Archivo→Abrir dirty ahora usa la protección existente; cerrar escena/ventana
requiere confirmar o guardar correctamente. Cancelaciones y fallo read-only
verificados. No suite completa, matriz nativa de exportadores ni impresión física.
Las filas siguientes conservan el alcance histórico de SC-09; esta actualización
no implica validación de todos los subsistemas.

## Capacidades y brechas

| Área | Core / fuente | Conexión GUI | Evidencia / límite | Trabajo |
|---|---|---|---|---|
| Mesh, validación y reparaciones | `libs/mesh-core/src/lib.rs`, `libs/scene/src/repair.rs` | `mesh_health.rs`, `repair_run.rs` | Parte de 955 pruebas históricas; reparación GUI no validada aquí | SC-29 |
| Escena jerárquica y recursos compartidos | `libs/scene/src/node.rs`, `scene.rs` | `scene_document.rs` es dueño único | Grupos/CSG/referencias en vocabulario no significan soporte de todas las operaciones | SC-28, SC-51 |
| Edits reversibles / clipboard | `libs/scene-edit/src/lib.rs` | `SceneDocument::apply_edit`, copy/paste/delete en viewport | Undo de gestos probado; roundtrip completo pendiente | SC-14, SC-25 |
| Identidad e invalidación espacial | `libs/mesh-spatial/src/lib.rs` | loader prepara; controlador consume | 57 pruebas integración históricas; conservar revisión y propiedad de cachés | SC-38 |
| Contacto real con cama | `MeshSpatial` + `viewport/bed_clearance.rs` | rotate/scale/gizmo/modal | Probado geométricamente y por Damian; traslación Z explícita libre | SC-03, SC-27 |
| Importar STL/OBJ/3MF | `libs/mesh-formats/src/lib.rs` | `scene_loader.rs`, `mesh_loader.rs`, `wiring.rs` | STL grandes probados; corpus multiformato pendiente | SC-24, SC-29 |
| Exportar geometría | `libs/mesh-formats/src/scene_writer.rs` | `scene_export.rs`, `wiring.rs` | OBJ/3MF conservan estructura; STL aplana; no equivalencia de formatos | SC-14, SC-52 |
| Contenedor 3MF | `libs/3mf/src/lib.rs` | vía project/mesh formats | Biblioteca existente, no rehacer contenedor | SC-14, SC-43 |
| Guardar/abrir proyecto 3MF | `libs/project-formats/src/lib.rs` | `scene_save.rs`, `scene_loader.rs`, Save/Save As en `wiring.rs` | Guardado atómico y stamps de revisión; no autosave de documento observado | SC-14, SC-30 |
| Slices persistidos y token | `libs/print-glue/src/fingerprint.rs` | `reloaded_slices.rs`, `slice_result.rs` | Revalida perfiles/geometría; AA no persistido y vuelve Off, según fuente | SC-31 |
| Selección de objetos | `NodeId` estable | `selection.rs`: **None/One**; `viewport/object_list.rs` | Lista plana derivada de hojas; no multiselección por asumir jerarquía del core | SC-25, SC-28 |
| Gizmo y transformaciones | affine f64 / vértices f32 | `gizmo_drag.rs`, `modal_transform.rs`, UI Slint | G/R/S, ejes mundo, confirm/cancel y undo probados; sin entrada numérica modal | SC-04, SC-26 |
| Cámara y proyección | `camera.rs` | `camera_controller.rs`, `picking.rs`, renderer | 22 tests previos y aprobación usuario; corregido hover Slint | SC-05, SC-06, SC-70 |
| Perfiles y resolución | `libs/print-profiles/src/lib.rs` | `profile_catalog.rs`, `project_profiles.rs`, forms/preferences | Ya hay librería, edición/import/export y valores heredados; calibración personal pendiente | SC-32, SC-57 |
| Unidades / build volume | `libs/print-primitives/src/lib.rs` | `build_volume.rs`, `slice_panel.rs` | mm, Z arriba, tamaño/resolución tipados; revisar unidades sin metadatos de STL | SC-27, SC-33 |
| Orquestación de impresión | `libs/print-glue/src/lib.rs` | `slice_projection.rs`, `export_print_file.rs` | Ya enlaza perfiles → schedule → slice → programa → writer | SC-33, SC-34 |
| Sliceado CPU y raster | `libs/sl1cer/src/api.rs`, raster | `slice_run.rs`, cancel/progress | Mallas manifold; AA Off/2/4/8; CSG y referencias **rechazados**, no evaluados | SC-33, SC-34, SC-51 |
| Visor de capas y exportación | `slice_result.rs`, `slice_projection.rs` | `slice_panel.rs`, `print_file_preview.rs`, `export_print_file.rs` | Conexión existente; preview/capa/export deben contrastarse en fixtures | SC-34, SC-55 |
| Writers de impresora | `libs/sla-formats/src/write.rs`, `printer_file_format.rs` | selección/export print | **GOO 3.0, CTB v5 y v5enc implementados**; GOO 5.1 rechazado. README desactualizado; no Anycubic writer observado | SC-19, SC-35, SC-36 |
| Jobs geométricos | `libs/sc-geom/src/lib.rs`, transport + generic dispatch | `support_gen_run.rs` | Worker, progreso, cancel token/recipe disponibles; verificar integración de cancel/stale al extender | SC-37 |
| Soportes actuales | `support-gen-trivial/src/trivial_support.rs` | Tools → Generate Support → paste de mallas undoable | Prismas hexagonales demo; anchors por AABB si no explícitos; espera artificial 2s. **No editor paramétrico** | SC-10, SC-11, SC-12 |
| Soportes DF portados | `libs/sc-geom/support-gen-df/src/pipeline.rs` (`generate_supports`, `support_soup`), adaptador `support-gen-df-sc` | `df_support_run.rs`: Tools → Generate Supports (DragonFruit), job cancelable, pega una malla deshacible | Paridad exacta con DragonFruit por bloque (`docs/sc/DF-PORT.md`); **sin prueba de Damian todavía**; ajustes por defecto y capa 0,05 fija; slice de la malla pendiente (SC-81) | SC-78, SC-79, SC-81 |
| Chalyplex | `support-gen-chalyplex/src/lib.rs` | No reexportado/conectado | Crate reservado vacío, no algoritmo disponible a reutilizar | SC-10, SC-15 |
| Hollow 3D, agujeros, blockers | Sin implementación de producto identificada en Rust/UI inspeccionados | Sin controles activos identificados | `sc-geom` menciona shell/offset en descripción, no demuestra implementación | SC-18, SC-39…SC-46 |
| Hollow 2D | Sin ruta activa identificada | Sin ruta activa identificada | Independiente de cavidad mesh; no tratar raster AA como hollow | SC-47 |
| Islas y autosupport real | Slicing ofrece contornos; demo no detector de islas por capas | Sin editor/detector de producto identificado | No afirmar que suggest_support_regions equivale a motor de cobertura | SC-17, SC-48…SC-50 |
| Ajustes y paths | `libs/app-paths/src/lib.rs` | `window_prefs.rs`, preferences bus | Ajustes de ventana no equivalen a recuperación automática del proyecto; namespace OS propio | SC-30, SC-59 |
| Localización y errores | `libs/message-catalog/src/lib.rs` | `i18n.rs`, `language.rs`, dialogs | Infraestructura existente; no dar por traducidos textos propios nuevos | SC-60, SC-61 |
| CLI y herramientas | `apps/slicer-cli`, profiles-converter, icon-gen | GUI usa servicios compartidos | CLI existente slice/convert/diagnose/fix/profiles y soporte demo; no motor independiente a reescribir | SC-56 |
| Fixtures y empaquetado | mesh/scene-fixtures, packages/ | runtime nativo | Harnesses previos parciales; plataformas distintas de Windows sin validar | SC-24, SC-54, SC-58, SC-69 |

Los nombres de fuente de GUI en la tabla son relativos a `apps/slicer-gui/src/`.
Las ausencias están limitadas al snapshot inspeccionado, no son afirmaciones sobre
desarrollo upstream futuro ni sobre otros slicers.

## Arquitectura a conservar

1. Documento autoritativo + IDs estables; mutations por edits reversibles.
2. Meshes fuente compartidas, transformaciones separadas; geometría derivada por
   revisión. No convertir soportes/hollow en mallas irreversibles como único dato.
3. Workers para preparación y geometría; preview liviano en gestos; resultados con
   revisión/identidad para descartar trabajos obsoletos.
4. Servicios del core compartidos entre GUI y CLI; no duplicar lógica en widgets.
5. Preview, exportación y sliceado con contrato común y pruebas cruzadas, sin
   confundir representaciones visuales con geometría imprimible.

## Siguiente secuencia recomendada

SC-24 corpus → SC-14 flujo guardar/abrir/historial → SC-10 contrato del primer hito
de soportes. Después modelo SC-11 y editor SC-12, con SC-37/38 para jobs/cachés.
Hollow tiene su propia especificación y preflight geométrico antes de elegir motor.
Esto no inicia esas tareas ni bloquea correcciones urgentes de la versión actual.

## Seguimiento SC-24 — corpus (posterior a esta auditoría)

Se completó el [corpus sintético](../../fixtures/sc-corpus/README.md): 25 casos,
28 assets, procedencia/licencia/unidades y resultados esperados independientes.
El harness nuevo usa las APIs existentes del core y verifica geometría, sondas de
contornos, formatos/proyecto y rechazos tipados. Dos generaciones fueron idénticas
byte a byte; [evidencia actual](CORPUS-VERIFICATION.json). Esta verificación nueva
no reemplaza la evidencia histórica de SC-09 ni cierra el flujo nativo de SC-14.
La GUI y su ejecutable aprobado no cambiaron; sólo se agregaron herramientas de
desarrollo y dos dependencias dev ya presentes en el lockfile (sin nuevas versiones).
