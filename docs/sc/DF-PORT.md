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

## Fase 1, bloque 1 — tipos, registro y primitivas (hecho)

Crate [`libs/sc-geom/support-gen-df`](../../libs/sc-geom/support-gen-df/)
(AGPL-3.0-or-later, `NOTICE` con atribución DF y three.js MIT, `COPYING`).
Un archivo Rust por archivo DF, con el origen en el encabezado.

- **Matemática three.js**: shim propio (`src/three/`) transliterado de three r181
  (`Vector3`, `Quaternion`, `Line3`, `Ray`, `MathUtils`) en lugar de glam, para
  conservar el orden de operaciones de three (`normalize` multiplica por
  `1/length`, `lerp`, `setFromUnitVectors`, `distanceSqToSegment`). `Math.max/min`
  conservan la semántica JS con NaN.
- **Datos**: `types.ts` completo con serde y los nombres de campo de DF; un
  snapshot DF se lee y se reescribe sin cambios. Colecciones en `IndexMap`
  (orden de inserción de un objeto JS, del que dependen los recorridos DF).
- **Registro**: los 8 descriptores con todos sus campos y todas las funciones
  derivadas. Las "ranuras" que DF llena al cargar (`register*`) quedan como
  listas fijas de pertenencia; el comportamiento se despacha directo cuando se
  porte cada tipo.

| Archivo DF | Rust | Estado |
|---|---|---|
| `types.ts` | `supports/types.rs` | completo |
| `supportTypeRegistry.ts` | `supports/support_type_registry.rs` | completo (ranuras como listas) |
| `history/actionTypes.ts` | `add_action`/`remove_action` en el registro | sólo nombres de acción |
| `constants.ts` | `supports/constants.rs` | completo |
| `autoSupport/constants.ts` | `supports/auto_support/constants.rs` | completo |
| `Curves/BezierUtils.ts` | `supports/curves/bezier_utils.rs` | completo |
| `SupportPrimitives/ContactCone/{types,contactConeUtils}.ts` | `support_primitives/contact_cone/` | lógica completa; el render queda fuera |
| `SupportPrimitives/ContactDisk/{contactDiskUtils,ContactDiskInteraction}.ts` | `support_primitives/contact_disk/` | lógica completa |
| `SupportPrimitives/Knot/{knotUtils,segmentEndpoints}.ts` | `support_primitives/knot/` | completo salvo la etiqueta de historial (UI) |
| `SupportPrimitives/Shaft/shaftTaperGeometry.ts` | `support_primitives/shaft/` | completo |
| `SupportPrimitives/Joint/jointUtils.ts` | `support_primitives/joint/joint_utils.rs` | `splitSupportShaft`, `findClosestSegment`; `moveJoint` espera al solver de restricciones y a `curveUtils` |
| `PlacementLogic/CollisionAvoidance.ts` | `placement_logic/collision_avoidance.rs` | completo sobre el trait `CollisionField` (las 3 consultas DF a `SDFCache`); falta portar `SDFCache` |
| `SupportTypes/Kickstand/types.ts` | `support_types/kickstand/types.rs` | tipos de datos; `KickstandBuildInput` va con el builder |

Diferencias deliberadas, ninguna en un camino normal: un `ContactDisk` en el
lugar de un socket de cono (DF lanza una excepción leyendo `normal`) se trata
como "sin contacto" y aplica el stub de respaldo; los ids que DF genera con
`uuidv4()` los provee quien llama, para poder correr determinista.

**Evidencia** (sin build de la GUI, sin escritorio):
- 41 tests portados de DF (`supportTypeRegistry`, `derivedTypeSubsets`,
  `contactDiskNib`, `jointInsertionKnotRemap`, parte de taper de
  `shaftTaperGeometry`) más los chequeos de consistencia que DF corre al cargar el
  registro.
- **Paridad bit a bit con DF**: `scripts/df-oracle/golden_primitives.ts` corre las
  funciones originales de DF sobre 150 entradas pseudoaleatorias deterministas
  por función (20 funciones, ~3.000 casos, incluidos contactos degenerados, normales
  nulas y perfiles legacy) → `tests/fixtures/df_primitives.json`;
  `tests/df_parity_primitives.rs` las corre en Rust y exige igualdad exacta
  (tolerancia 0). Un cambio de 1e-7 en una constante o de un ulp en `normalize`
  hace fallar el test (comprobado).
- `cargo clippy -p support-gen-df --all-targets` (workspace `deny`) y `cargo fmt` limpios.

## Fases siguientes

1. **Transliteración 1:1** en un crate nuevo que replica `src/supports/`:
   tipos/registro → primitivas y builders (Trunk, Branch, Leaf, Twig, Stick,
   Stump, Brace, Kickstand) → `PlacementLogicV3` → `autoSupport` → `autoBracing`
   → `Rafts`. Raycasts y distancias DF sobre el BVH de SC. Se portan los tests
   de DF y cada módulo se compara con el oráculo antes de avanzar.
2. **Detección de islas completa**: voxel (TS), overhangs y mínimos (Rust DF, ya
   probados en la rama anterior) sobre raster/topología SC.
3. **Integración GUI**: jobs cancelables, preview, un undo, 3MF y salida SC;
   luego interacciones (arrastre de joints/knots, snapping, multiselección) en
   Slint con la lógica DF debajo. Gestos los prueba Damian.
