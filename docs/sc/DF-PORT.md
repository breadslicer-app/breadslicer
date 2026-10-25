# Port nativo de soportes DragonFruit → SC

Rama `sc/df-supports`, creada desde `sc-project-safety-20261022` (`820ffd37`):
editor/navegación aprobados y protección de proyectos, sin la implementación de
soportes anterior. `sc/development` queda intacta como referencia.

## Objetivo

Implementación **nativa en Rust** de los soportes de DF dentro de la arquitectura
SC (escena/documento, SceneEdit/undo, jobs, 3MF, sl1cer, BVH, renderer).
Se translitera el código TypeScript de DF archivo por archivo, conservando
nombres, estructura y algoritmos; no se embebe DF ni un runtime JS en el producto.
DF y SC son AGPL-3.0: se preservan autoría y avisos de Open Resin Alliance y
SynrgStudio en cada archivo portado.

Lo anterior (`sc/development`, T-028–T-054) fue una reimplementación propia con
diferencias deliberadas, no un port: no se reutiliza como lógica de soportes.
La infraestructura genérica de esa rama (consultas espaciales preparadas,
`sl1cer` prepared/z_bins, fragmentos de scene-edit) se trae sólo cuando el port
la necesite.

## Fase 0 — oráculo DF (hecho)

DF corre su propio pipeline Auto (`scripts/bench-auto-supports.ts` →
`runAutoPlaceRequest`, el mismo camino del worker de la app) desde una copia
descartable en `.local/df-oracle/`. La referencia sólo se lee. Herramientas:
[`scripts/df-oracle/`](../../scripts/df-oracle/) (`pose_stl.py` para posar STL,
`run_oracle.sh` para ejecutar). DF revisión `f331fc555abb31c4bcb28d1a358edb887f0afe07`,
Node 26.4.0.

Límite: sin host Tauri, DF detecta islas sólo por vóxeles (`pxMm` 0.1, capa
0.05 mm). Las familias `scan_overhangs` y `scan_mesh_minima` (Rust/Tauri) no
participan, así que la app real tiene más islas. Los modelos posados son
privados y quedan en `.local/df-oracle/corpus-*` (no versionados).

| Modelo / pose | Islas | Contactos | Tipos colocados | Refuerzos | Cobertura | Pipeline |
|---|---|---|---|---|---|---|
| cubo 30 mm, rot. 30° X y Y, Zmin 10 | 2 | 57 | 43 trunk, 14 branch | 173 brace, 1 kickstand | 35.7 % (2 islas sin cubrir) | 0.34 s |
| Wolverine, pose del `wolverine_repaired.3mf` (≈33° Y) | 22 | 43 | 8 trunk, 8 branch, 5 leaf, 10 twig, 1 stick | 14 brace, 6 kickstand | 30.6 % (20/22 islas) | 4.5 s (30 s total con carga y detección) |
| Wolverine centrado, Zmin 10 | 7 | 8 | 5 trunk, 1 leaf, 1 twig | 16 brace, 1 kickstand | 100 % | 3.9 s (26 s total) |

Salidas completas: [evidence/df-oracle/](evidence/df-oracle/).
SHA256 de los STL posados: cubo `6aa995eb…07c5cf`, Wolverine 3MF `c6da2a21…b08700ce`,
Wolverine centrado `5e4e7573…21eeb77489`.

Comparación con la implementación anterior de SC en las mismas poses de
diagnóstico: el cubo no terminaba el fan en 600 s (debug) y Wolverine fallaba
antes de generar soportes, en el stitching de `sl1cer` a Z 32.5 mm
("2 dangling endpoints"). **Ese fallo de stitching es de la infraestructura SC
y sigue presente en esta rama**: el detector de vóxeles de DF no pasa por él.
Hay que resolverlo aparte de los soportes.

## Fases siguientes

1. **Transliteración 1:1** en un crate nuevo que replica `src/supports/`:
   tipos/registro → primitivas y builders (Trunk, Branch, Leaf, Twig, Stick,
   Stump, Brace, Kickstand) → `PlacementLogicV3` → `autoSupport` → `autoBracing`
   → `Rafts`. Matemática three.js sobre un shim delgado (glam); raycasts DF
   sobre el BVH de SC. Se portan los tests de DF y cada módulo se compara con el
   oráculo antes de avanzar.
2. **Detección de islas completa**: voxel (TS), overhangs y mínimos (Rust DF, ya
   probados en la rama anterior) sobre raster/topología SC.
3. **Integración GUI**: jobs cancelables, preview, un undo, 3MF y salida SC;
   luego interacciones (arrastre de joints/knots, snapping, multiselección) en
   Slint con la lógica DF debajo. Gestos los prueba Damian.
