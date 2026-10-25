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
| `SupportPrimitives/Joint/jointUtils.ts` | `support_primitives/joint/joint_utils.rs` | `splitSupportShaft`, `findClosestSegment`, `moveJoint` (bloque 2) |
| `PlacementLogic/CollisionAvoidance.ts` | `placement_logic/collision_avoidance.rs` | completo sobre el trait `CollisionField`, que implementa `SdfCache` (bloque 2) |
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

## Fase 1, bloque 2 — SDF, BVH, restricciones, curvas y `moveJoint` (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| `PlacementLogic/Pathfinding/SDFCache.ts` | `placement_logic/pathfinding/sdf_cache.rs` | completo: tabla de celdas en `f32` con su capacidad y el `Map` de desborde en `f64`, cuantización con `Math.round` de JS, firma por normal de cara, marcha con cota, mapa de columnas |
| `PlacementLogic/Pathfinding/SDFCachePool.ts` | `placement_logic/pathfinding/sdf_cache_pool.rs` | completo; el pool es un valor del worker en vez de un global, y avisa de deriva de matriz |
| `PlacementLogic/Pathfinding/ColumnClearanceMap.ts` | `placement_logic/pathfinding/column_clearance_map.rs` | completo (`buildMs` no se registra) |
| `PlacementLogic/ShaftAngleConstraint.ts` | `placement_logic/shaft_angle_constraint.rs` | completo |
| `PlacementLogic/JointConstraintSolver.ts` | `placement_logic/joint_constraint_solver.rs` | completo; el store se pasa como argumento |
| `Curves/curveUtils.ts` | `curves/curve_utils.rs` | completo |
| three-mesh-bvh 0.8.3 (`MeshBVH`, `closestPointToPoint`) | `three_mesh_bvh/` | construcción `CENTER` y búsqueda del punto más cercano; los raycasts vienen con su primer uso |

**BVH.** El BVH no es el de `mesh-spatial`. En esta rama `mesh-spatial` sólo tiene
clústeres; el BVH SAH de `sc/development` depende del código de soportes viejo
(`sc_scene::supports`) y no hace la búsqueda "más cercano primero". Además, DF
depende de *su* árbol: three-mesh-bvh reordena el índice de triángulos, el
`faceIndex` se refiere a ese orden, y ante un empate exacto el orden de recorrido
decide qué cara (y qué normal) firma la distancia. Por eso se portó three-mesh-bvh
(MIT) sobre las posiciones de la malla SC: mismo árbol, mismo índice reordenado,
mismas cajas `f32`. Wolverine (1,9 M triángulos): construcción 386 ms, 725
columnas de colisión en 58 ms (release, `tests/wolverine_sdf_scale.rs`, ignorado
sin el STL privado).

**Trigonometría de V8.** `Math.sin`/`Math.cos` de V8 14.6 (Node 26) son el
fdlibm 5.3 original, no glibc ni musl: los kernels originales coinciden en 9.176
de 9.176 argumentos sin reducción, y `libm`/la std de Rust difieren en ~1 %.
`src/js_math.rs` porta `k_sin`, `k_cos` (con el término `qx`) y
`__ieee754_rem_pio2` (tabla `npio2_hw` y constantes verificadas contra π de alta
precisión). `acos` y el resto ya eran fdlibm = crate `libm`.
`tests/js_math.rs`: 20.008 argumentos de V8, de 1e-9 a 9e5, idénticos bit a bit
(los que superan 2^19·π/2 usan la reducción Payne–Hanek de `libm`; los ángulos de
DF no llegan ahí).

Diferencias deliberadas, documentadas en el código:
- `updateCurvesAtJoint` modifica en el lugar el segmento Bézier del trunk de
  entrada; el trunk devuelto refleja todas esas escrituras, pero el valor de
  entrada de Rust no cambia.
- `removeSegmentCurve` deja el `bias` en el segmento recto; se conserva
  (`SegmentShape::Straight::stray_bias`) para que el registro sea igual.
- `moveJoint` sin `maxStandoffMm` con malla: DF propaga `undefined` → NaN; el port
  propaga NaN igual.

**Evidencia:** `scripts/df-oracle/golden_sdf.ts` → `tests/fixtures/df_sdf.json`
(5 mallas: cubo, torus knot, torus knot movido/rotado/escalado, lámina abierta
plegada, malla `ties` con centros justo en los planos de corte). Por malla: índice
reordenado, 120 `closestPointToPoint` con umbrales, ~700 consultas SDF en orden de
llamada (exacta, cuantizada, acotada, trilineal, gradiente, `within`,
`segmentBlocked`, activación del mapa de columnas) con los contadores
`cellReads`/`bvhQueries` y el tamaño de la caché, y 200 consultas de columnas.
Además 300 casos de `clampShaftAngle`, `solveJointConstraint`, las 7 operaciones de
curvas, `moveJoint` (con/sin malla, modo curva, `skipContactConeSolve`, ángulo
máximo) intercalados con 300 chequeos de colisión sobre la misma caché.
`tests/df_parity_sdf.rs` exige **igualdad exacta** en todo. Sensibilidad
comprobada: guardar la caché en `f64` falla en la op 373 del cubo; cambiar el
desempate de la partición (`>=` → `>`) falla en el índice de `ties`. Quitar el
épsilon de las cajas no cambia ningún resultado (sólo poda) y no se detecta, lo
cual es correcto.

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 49 tests más
1 de escala ignorado.

## Fase 1, bloque 3 — raycasts, colisión, eje de cono, settings y builders (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| three-mesh-bvh `raycast`, `raycastFirst`, `intersectRay`, `intersectTri`, `acceleratedRaycast` | `three_mesh_bvh/raycast.rs` (`BvhMesh::intersect_object`) | completo: rayo a espacio local con la matriz de mundo, lados front/back/double, orden estable por distancia; sin interpolación de normales/uv (nadie las lee) |
| three.js `Ray.intersectTriangle`, `Triangle.getNormal`, `Matrix4`/`transformDirection`, `applyAxisAngle` | `three/` | completo |
| `PlacementLogic/CollisionUtils.ts` | `placement_logic/collision_utils.rs` | completo (bigotes, puente corto, Bézier) |
| `PlacementLogic/ConeAxisPolicy.ts` | `placement_logic/cone_axis_policy.rs` | completo |
| `PlacementLogic/StandardPlacement.ts` | `placement_logic/standard_placement.rs` | completo |
| `PlacementLogic/Grid/gridMath.ts` | `placement_logic/grid/grid_math.rs` | completo |
| `Settings/{types,defaults,supportSettingsCodec}.ts`, `Settings/state.ts` (normalización) | `settings/` | completo; el store global, sus listeners y `localStorage` son de la GUI |
| `autoSupport/settings.ts`, `autoBracing/settings.ts` | `auto_support/settings.rs`, `auto_bracing/settings.rs` | completo (`clampNumeric` compartido en `numeric_constraint.rs`) |
| `autoSupport/parameterSizing.ts` | `auto_support/parameter_sizing.rs` | `applySizingOverridesToSettings`; el sizing empírico va con el Auto |
| `rendering/SupportBuilder.tsx` | `rendering/support_builder.rs` | `SupportData` y `supportDataForEntity` (el render es de la GUI) |
| `SupportTypes/Trunk/{trunkBuilder,trunkConstructionJoints,trunkRouteTypes,trunkRouteResolution}.ts` | `support_types/trunk/` | completo; `buildTrunkData` con malla llama a `SmartPlacementV3`, que se porta con el router y alimenta el mismo `buildTrunkDataFromPlacement` |
| `SupportTypes/Branch/branchBuilder.ts` | `support_types/branch/branch_builder.rs` | completo (búsqueda de socket 4×12×4 con puntaje y preferencia de inclinación, `remapBranchGeometryIds`) |
| `SupportTypes/Leaf/leafBuilder.ts` | `support_types/leaf/leaf_builder.rs` | completo |

**`Math` de V8, auditado entero.** Fixtures de Node 26 para cada función que usa el
código de soportes: `acos`, `asin`, `atan`, `atan2`, `exp`, `tan` (fdlibm = crate
`libm`, 100 %), `sin`/`cos` (fdlibm original, `js_math`), `hypot` (port del
`MathHypot` de V8: normalización y suma de Kahan, no `sqrt(x²+y²)`) y
`Number.prototype.toFixed` (redondea la magnitud y desempata alejándose del cero;
el formateo de Rust desempata a par). `x ** 2` es siempre `x * x` en V8. `pow`,
`log` y `cbrt` difieren de `libm`/la CRT en algunos casos y todavía no los usa
código portado; se resuelven con el sizing del Auto.

**Ids deterministas.** El sandbox del oráculo reemplaza `uuid` por un contador
(`scripts/df-oracle/uuid_stub.mjs`, activado con `DF_ORACLE_DETERMINISTIC_UUID`),
y los builders de Rust reciben el mismo contador: las entidades se comparan
enteras, ids incluidos.

**Evidencia:** `scripts/df-oracle/golden_builders.ts` → `tests/fixtures/df_builders.json`:
320 casos de eje de cono (3 modos), 160 raycasts sobre una malla movida/rotada/escalada
(lados, near/far, `firstHitOnly`), 320 chequeos de `CollisionUtils`, 160 normalizaciones
de settings con valores fuera de rango (+ hex de entrada/salida y decodificación),
160 de `gridMath`, 160 `StandardPlacement`, 160 trunks (sin malla y desde
colocaciones autoradas con juntas, nodos de grilla, errores; la mitad contra el
campo de colisión), 160 branches (con/sin malla, settings propios, perfil de punta,
reconstrucción y `remapBranchGeometryIds`) y 160 leaves. Puntas sobre la superficie
real del torus knot y en el aire. `tests/df_parity_builders.rs`: **igualdad exacta**.
Sensibilidad comprobada: cambiar un peso del puntaje del branch (6.75 → 6.76) o el
orden de hijos de `raycastFirst` falla. Para `toFixed` se agregó un fixture con
empates construidos (`tests/fixtures/v8_to_fixed.txt`), que encontró un error del
port (desempate negativo hacia el cero): 146 casos, corregido.

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 55 tests más
1 de escala ignorado.

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
