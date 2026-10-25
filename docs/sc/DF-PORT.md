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
| `SupportTypes/Trunk/{trunkBuilder,trunkConstructionJoints,trunkRouteTypes,trunkRouteResolution}.ts` | `support_types/trunk/` | completo; `buildTrunkData` con malla es `build_trunk_data_routed` (bloque 4) |
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

## Fase 1, bloque 4 — router V3 y `buildTrunkData` con malla (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| `PlacementLogicV3/EscapeJointSearch.ts` | `placement_logic_v3/escape_joint_search.rs` | completo: caminata a 45° por dirección, búsqueda en la red por anillos ordenados por desvío, presupuesto de 900 sondas, abanico de direcciones |
| `PlacementLogicV3/SmartPlacementV3.ts` | `placement_logic_v3/smart_placement_v3.rs` | completo: caída recta con desvíos de cono, una diagonal + caída vertical, volumen de raíces, memos, reintento sin grilla por la regla de forma, `RouterStats` |
| `PlacementLogic/smartPlacementSearchUtils.ts` | `placement_logic/smart_placement_search_utils.rs` | geometría completa; los tipos de nodo y el orden de cola del A* retirado no tienen llamador y no se portan |
| `PlacementLogic/Grid/nearestCandidateNodeKeys.ts` | `placement_logic/grid/nearest_candidate_node_keys.rs` | completo |
| `buildTrunkData` con `mesh` | `trunk_builder::build_trunk_data_routed` | completo; `TrunkRouter { sdf, matrix_world, stats }` y la misma `buildTrunkDataFromPlacement` contra ese campo |

**Diferencias deliberadas, ninguna cambia una colocación.** La malla es su campo de
distancia: el llamador pasa el `SdfCache` del pool y la matriz de mundo
(`refreshMatrix` lee `mesh.matrixWorld`). Settings y contadores entran como
argumentos (`getSettings()` y el `routerStats` global). El prewarm de mapas de
columna de los presets fijados corre en `requestIdleCallback`, que fuera del hilo
principal del navegador no hace nada (worker de DF y Node); la GUI puede llamar
`SdfCache::prewarm_column_map`. No se produce el snapshot del overlay de debug ni
las marcas de perf. Las dos consultas que DF hace sólo para ese overlay
(columna recta y raíces bajo el socket) se mantienen, porque cachean.

**Evidencia:** `scripts/df-oracle/golden_v3.ts` → `tests/fixtures/df_v3.json`:
300 búsquedas (libres y en red) contra el modelo analítico de cajas de los tests
de DF, con predicado de base disperso y opciones fuera de rango; 100 abanicos;
60 anillos de claves (claves malformadas incluidas); 400 casos de utilidades; y
703 llamadas al router en orden — `calculateSmartPlacementV3` (con
`ignoreGrid`, `isPreview`, islas chicas) y `buildTrunkData` con malla (overrides,
`ignoreGrid`) — sobre cinco escenas: torus knot, la mandíbula y la pared de los
tests de DF, una mesa con cajón debajo y un knot movido/rotado que deriva a mitad
de la corrida. Un campo por malla compartido en el orden de DF y los contadores
del router comparados después de cada llamada. Resultado: 160 rutas de una junta,
~150 caídas rectas, 26 reintentos sin grilla, rechazos por colisión y ángulo;
búsquedas `found`/`never-cleared`/`probe-budget`/`no-direction`.
`tests/df_parity_v3.rs`: **igualdad exacta**, más los 7 tests de
`smartPlacementV3.test.ts` portados.

Sensibilidad: 15 mutaciones fallan (desempate de la caminata, orden y sentido del
abanico, orden de desvíos, tolerancia de base recta, muestras de perímetro, claves
de memo, cubetas de sondas, presupuesto de nodos, tolerancia del techo diagonal,
pendiente del ajuste por largo). Sobreviven las equivalentes: la regla de forma
sobre la cadena recta (la base queda a ≤0.05 mm del socket, siempre vertical), la
holgura de la pata vertical (la base cae bajo la junta) y el techo de los conos
desviados (la inclinación de la junta es siempre la constante de 45°).

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 63 tests más
1 de escala ignorado.

## Fase 1, bloque 5 — builders Twig, Stick, Stump, Kickstand y Brace (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| `SupportTypes/Twig/{twigBuilder,twigTaper,twigJointStandoff,twigVerticality}.ts` | `support_types/twig/` | completo; la regla de diámetro de knot registrada es `twig_knot_diameter` |
| `SupportTypes/Stick/{stickBuilder,stickVerticality}.ts` | `support_types/stick/` | completo (dos pasadas de eje, cono acortado al hueco libre por rayos, tramo libre contra el campo, orden de extremos) |
| `SupportTypes/shaftVerticality.ts` | `support_types/shaft_verticality.rs` | completo |
| `twigRegistration.ts`/`stickRegistration.ts` (`registerContactBridgeBuilder`) + `buildContactBridge` | `support_types/contact_bridge.rs` | completo: gates de verticalidad (45° twig, 20° stick), exención manual |
| `SupportTypes/Stump/{stumpBuilder,stumpAutoPlacement}.ts` | `support_types/stump/` | completo: override de contacto con `STUMP_BELOW_ROOT` y colisión de cono, joint y raíz contra la distancia exacta |
| `SupportTypes/Kickstand/{kickstandBuilder,kickstandRules,kickstandSettings}.ts` | `support_types/kickstand/` | completo, con `toKickstandPreviewData`; un host inválido es `Err` (DF lanza) |
| `SupportTypes/Brace/braceSpanDiameters.ts`, `autoBracing/braceDiameter.ts` | `support_types/brace/`, `auto_bracing/brace_diameter.rs` | completo |

**Lo que no es builder y va en otro bloque.** Brace no tiene builder en DF: el
brace manual lo escribe `BracePlacementController.tsx` y el automático el
planificador de auto-bracing; su settle (`braceSettle.ts`) es el recálculo de knots
del store. `kickstandStabiliser.ts` (595 líneas) es el generador de auto-bracing.
Los grupos de export (`registerSupportExportGroup`) van con el exportador;
renderers, proxies, marquee, drag preview y placement state son de la GUI.

**Malla.** Los builders reciben `ModelMesh { field, rays }` (`supports/model_mesh.rs`):
DF les da un `THREE.Mesh` y cada chequeo elige su vista, el campo de distancia del
pool (`isShaftBlocked`, stump) o el raycaster (`checkShaftCollision`,
`checkShortBridgeCollision`); acá viajan juntas y cada builder lee la que lee DF.
El stump lee además los settings de raft en DF, para valores que no usa (offset
vertical fijo en 0); no se piden.

**Evidencia:** `scripts/df-oracle/golden_builders5.ts` → `tests/fixtures/df_builders5.json`
(entrando por `state.ts`, como los tests de DF, para que corran las registraciones):
2040 llamadas en orden — 271 twigs, 483 sticks, 200 bridges registrados (twig,
stick y un tipo sin builder; manuales y automáticos), 241 stumps, 544 overrides
de stump, 301 kickstands (hosts inválidos, overrides de layout fuera de rango,
columnas autoradas, límite de ángulo) — sobre un torus knot, una escena de techo
y piso con poste (también movida/rotada) y una escena baja con labios y columnas.
Además de 900 llamadas aleatorias hay barridos dirigidos a los umbrales: puentes y
kickstands a distancias crecientes del poste, contactos en paredes laterales,
sticks cortos, empates de orden, stumps acercándose a columnas bajo dos labios.
Más 200 layouts, 150 tapers (degenerados y casi degenerados) y 200 brace spans.
`tests/df_parity_builders5.rs`: **igualdad exacta**; `tests/builders5_df_tests.rs`
porta `manualBridgeCant`, `twigVerticality`, `twigSizing` y `kickstandColumnShape`.

Sensibilidad: 24 de 26 mutaciones fallan. Sobreviven el tope de longitud del cono
del stump (equivalente: con eje descendente coincide con la longitud estirada) y
el tope de 5 muestras por cuerpo del stump, que sólo cambia el veredicto en
contactos al borde. La primera tanda aleatoria dejaba vivas 10 de 19; los
barridos se agregaron por eso.

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 69 tests más
1 de escala ignorado.

## Fase 1, bloque 6 — autoSupport

El bloque va en tres partes, cada una con su oráculo: **6a** los pases que llama
el orquestador, **6b** la colocación en grilla, el puente de cavidad, el perfil de
diámetros del bosque y las operaciones del store, **6c** `autoPlace.ts` de punta a
punta. El auto-bracing es su propio bloque: el orquestador lo saltea con
`debugSkipAutoBracing`, la costura que DF ya tiene.

### `Math.pow`, `Math.log`, `Math.cbrt` de V8 (hecho)

Medido contra Node 26 / V8 14.6 en `tests/fixtures/v8_pow_log.txt` (20000 de cada
uno, argumentos con forma de sizing y aleatorios amplios):

- **`log`** es el `__ieee754_log` de fdlibm 5.3 (con su elección de fórmula por
  `i | j`); el crate `libm` trae la reescritura de musl, que difiere en ~0,05 %.
  Portado en `js_math::log`: **20000/20000**.
- **`cbrt`** es el `s_cbrt.c` de FreeBSD (semilla por bits, polinomio a 23 bits, un
  Newton); `libm` trae el cbrt correctamente redondeado de core-math, que V8 14.6
  no usa (91 %). Portado en `js_math::cbrt`: **20000/20000**.
- **`pow`**: V8 14.6 compila `math::pow` con `--use-std-math-pow` por defecto: los
  casos especiales de JS, `y == 2` como `x*x`, `y == 0.5` como `sqrt`, y si no el
  `pow` del runtime de C. El último bit es del CRT: el UCRT estático de node.exe y
  la `ucrtbase.dll` del sistema discrepan en 5 de 20000, en un ulp. `js_pow` sigue
  el camino de V8 sobre el `pow` de la plataforma (en Linux ambos serían glibc).
  El test admite ≤10 diferencias de exactamente un ulp; `main` de V8 ya pasó a
  LLVM-libc, así que un Node futuro vuelve a mover este bit.

### 6a — pases de autoSupport (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| `autoSupport/parameterSizing.ts` | `auto_support/parameter_sizing.rs` | completo: `sizeParameters`, factores de modelo y altura (con `js_pow`/`js_log`), `presetForArea`; las bandas se resuelven por `SizingBands` |
| `Settings/presets.ts` (bandas de fábrica) | `SizingBands::factory` | las tres de fábrica; el store de presets y `localStorage` son de la GUI |
| `autoSupport/loadBudget.ts` | `auto_support/load_budget.rs` | completo |
| `autoSupport/contactTipCap.ts` | `auto_support/contact_tip_cap.rs` | completo |
| `autoSupport/coverage.ts` | `auto_support/coverage.rs` | completo |
| `autoSupport/candidateGeneration.ts` | `auto_support/candidate_generation.rs` | completo |
| `autoSupport/minimaReinforcement.ts` | `auto_support/minima_reinforcement.rs` | completo |
| `autoSupport/poseStability.ts` | `auto_support/pose_stability.rs` | completo |
| `autoSupport/stabilization.ts` | `auto_support/stabilization.rs` | completo (sin el log de calibración) |
| `autoSupport/gridPlacement.ts` | `auto_support/grid_placement.rs` | completo |
| `autoSupport/supportBlockers.ts` | `auto_support/support_blockers.rs` | la consulta de contacto; el pincel, su store, historia y hover son de la GUI |
| `Rafts/Crenelated/geometry/convexHull2d.ts`, `utils/math.ts`, `volumeAnalysis/Islands/{types,voxelFootprint}.ts`, `Matrix3` de three | `rafts/…`, `utils/math.rs`, `volume_analysis/islands/`, `three/matrix3.rs` | lo que estos pases leen |

**Diferencias deliberadas.** DF lee dos settings distintos en estos pases: el
`autoSettings` del run y el global `getSettings().autoSupport` (preset de sizing,
topes de escala, margen de punta); acá entran los dos explícitos, donde DF lee
cada uno. El preset se resuelve contra la tabla que da el llamador (el diseño del
worker de DF). La malla es `AutoSupportMesh { mesh, side, blocked }`: el campo y
los rayos, el lado del material con que raycastea, y la máscara de blockers.

**Encontrado al portar.** El "espejo" `SIZING_BANDS` de DF no es exacto: el preset
de fábrica `structure` hereda la penetración por defecto (0,1 mm), no 0; las bandas
salen de los presets. three-mesh-bvh arma una raíz por grupo de geometría (un
`BoxGeometry` trae seis); STL y 3MF no traen grupos y el BVH del port arma una sola
raíz, así que los oráculos limpian los grupos de sus mallas de prueba.

**Evidencia:** `scripts/df-oracle/golden_autosupport_a.ts` → `tests/fixtures/df_autosupport_a.json`:
615 llamadas sobre seis mallas (knot, mesa con cajón, cubo sobre un vértice, cubo
sobre una arista, tablón inclinado, cono colgante; dos con máscara de blockers),
islas sintéticas con forma real (huellas de vóxeles de 0,25 mm en rectángulo,
disco, anillo, L, línea y mota; caras de la malla debajo; loops de perímetro),
settings y settings de auto-support aleatorios y fuera de rango, y regiones
grandes que desbordan el tope de candidatos por región. Estabilización en las
seis mallas, con y sin raft (dispara en las cuatro inestables). `tests/df_parity_autosupport_a.rs`:
**igualdad exacta** (los infinitos llegan como texto y se comparan aparte).
Sensibilidad: 24 de 25 mutaciones fallan; sobrevive la pertenencia por bucket
del dedup, que es equivalente (la compuerta de distancia ya la implica).

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 71 tests más
1 de escala ignorado.

### 6b — grilla, puente de cavidad, perfil del bosque, borrador (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| `PlacementLogic/Grid/{gridPlacement,types}.ts` | `placement_logic/grid/grid_placement.rs` | completo: índice de hosts por nodo (raíz y contacto), hosts cerca del nodo, override del stump, adjunción por knot de arriba hacia abajo (leaf o branch, con piso de diámetro y repliegue a la banda), vecinos, compuerta de colisión del trunk, snap y rechazos |
| `supportTypeRegistry.ts` (`PlacedSupport`, `placementOfResolved`) | `support_type_registry.rs` | la forma genérica de una colocación; el override del stump la devuelve también |
| `SupportTypes/Trunk/useTrunkPlacement.ts` (`buildCavityBridge`) | `support_types/trunk/cavity_bridge.rs` | completo; el hook de React es de la GUI |
| `SupportTypes/Trunk/TrunkReplacement/maxConnectedDiameter.ts` | `support_types/trunk/trunk_replacement/max_connected_diameter.rs` | `memberDiameterOf`, `applyDiameterToTrunk`, el perfil escalonado y el resize del bosque; la caminata del grafo conectado va con el store |
| `PlacementLogic/PlacementUtils.ts` (`calculateSmoothedNormal`) | `placement_logic/placement_utils.rs` | completo (con `getBarycoord` de three) |
| `autoSupport/supportDraft.ts` | `auto_support/support_draft.rs` | completo |

`decideGridPlacement` lee dos settings, como DF: los de la decisión
(`args.settings`) y los globales que leen los builders de miembros y la sonda de
colisión del branch.

**Evidencia:** `scripts/df-oracle/golden_autosupport_b.ts` → `tests/fixtures/df_autosupport_b.json`:
16 mundos de 30 pasos sobre cuatro mallas (knot, mesa, techo/piso con poste, una
escena baja rotada): candidato de trunk (ruteado o no), decisión de grilla y
commit al borrador en un snapshot que crece, con tips agrupados sobre los
anteriores, pasos de búsqueda gruesos y medio de los mundos cambiando el
espaciado a mitad de corrida (raíces entre nodos). 480 decisiones (166 trunks,
41 branches, 3 leaves, 38 stumps; rechazos por colisión, sin adjunción y stump
bajo la raíz), el snapshot final, el resize del bosque y los diámetros de
miembro; 163 puentes de cavidad y 137 normales suavizadas. Un solo contador de
ids para toda la corrida en ambos lados. `tests/df_parity_autosupport_b.rs`:
**igualdad exacta**. Sensibilidad: 13 de 16 mutaciones fallan; sobreviven el
`max` de la junta inferior (equivalente: la junta es compartida y el pase de la
superior ya la cubre) y dos umbrales de casi-roce que el fixture apenas toca (la
tolerancia de medio paso de `hostsNearNode` entre 0,4 y 0,5, y el radio de la
compuerta del puente).

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 72 tests más
1 de escala ignorado.

### 6c — `autoPlace.ts` de punta a punta (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| `autoSupport/autoPlace.ts` (helpers) | `auto_support/auto_place/helpers.rs` | contexto de la corrida (`PlanMesh`, `PlanCtx`: settings vivos, bandas, malla con su SDF del pool, máscara de blockers, un contador de ids, contadores del router), resolución de normales con disco de rayos, filtro de ya-soportados, colisiones de cono y branch, capacidad de adjunción, host de merge, branch hospedado, pool de muestras de fan, re-hosteo de knots legados, sync cono/fuste y el cull de huérfanos |
| `autoSupport/autoPlace.ts` (`fanLeafToHost`, `buildConsolidationBranch`) | `auto_support/auto_place/fan.rs` | completo: leaf o branch sobre la muestra más empinada (o más cercana), tier de rescate ancho, motivos de rechazo con distancias |
| `autoSupport/autoPlace.ts` (`placeOneCandidate`) | `auto_support/auto_place/place.rs` | completo: normal, fans grid→isla y de overhang, merge gridless (leaf o branch), trunk dimensionado y ruteado, diferimiento al stump, fan de cavidad y puente modelo-a-modelo, guarda de pared lateral, decisión de grilla, commit y reintento sin grilla |
| `autoSupport/autoPlace.ts` (`computeAutoSupportPlan`) | `auto_support/auto_place/plan.rs` | completo salvo auto-bracing: candidatos (estabilización, refuerzo de mínimos, grilla de densidad), dedup, filtro, colocación, consolidación en árboles, gap-fill, fanning de hojas, branches de cobertura en islas planas, resize del bosque, re-hosteo, cull, sync de conos, informe del bosque y presupuesto de carga |
| `autoSupport/autoPlace.ts` (`buildForestReport`) y `types.ts` | `auto_support/auto_place/report.rs` | completo; `forestReportToText` es de la GUI |

Lo que DF lee de sus stores entra explícito: el estado comprometido
(`getSnapshot()`, que lee el gap-fill: DF lo deja así a propósito, ver su
`docs/dev/backlog.md`), el modo de raft del modelo y los settings vivos en el
contexto. El auto-bracing no está portado: si la corrida no pide
`debugSkipAutoBracing`, el plan devuelve el bosque sin brazos y lo marca en
`bracing_skipped`. Logs, tiempos, progreso y el rollback del store son de la
consola y el modal de DF y no se reproducen (aquí nada se compromete).

**Hallazgo DF:** la re-colocación de huérfanos es código muerto. El filtro busca
`'host trunk culled'` y el cull escribe `'trunk culled (blocked)'`, así que los
miembros de un host recortado nunca se vuelven a colocar. El port lo reproduce
tal cual; el arreglo queda para después del porteo como SC-75 en `ROADMAP.md`.

**Evidencia:** `scripts/df-oracle/golden_autosupport_c.ts` → `tests/fixtures/df_autosupport_c.json`:
140 corridas de `computeAutoSupportPlan` sobre siete mallas (knot, mesa, techo/piso
con poste, escena baja rotada, cono colgante, cubo sobre una arista, tablón
inclinado; dos con máscara de blockers, tres sin raft) con settings y overrides de
auto-support aleatorios (grilla on/off, estabilización y refuerzo de mínimos,
radios y ángulos de fan, tope de adjunciones), de 0 a 14 islas sintéticas con
forma real, y un tercio de las corridas construyendo sobre el bosque de una
corrida anterior tal como lo guarda el store tras el commit (`setSnapshot`
normaliza las colecciones en `supports`; un `cloneSupportState` del borrador
pierde los stumps que solo están en la vista). Resultado: 655 trunks, 751 branches,
388 leaves, 168 sticks, 109 stumps y 37 twigs; 1160 rechazos de construcción, 303
de grilla y 18 por colisión; 18 reintentos sin grilla; 205 puentes de cavidad y
fans de cavidad logrados; consolidación con todos sus motivos de rechazo; huérfanos
por bloqueo, cruce, host bloqueado y host faltante; estados `placed`,
`no-candidates`, `already-supported` y desactivado. Se comparan el bosque, la
analítica completa (informe del bosque, presupuesto de carga, `sizingDebug`) y
el resultado. Un contador de ids y un SDF por malla para toda la corrida.
`tests/df_parity_autosupport_c.rs`: **igualdad exacta** en las 140 corridas.
Sensibilidad: 17 de 22 mutaciones fallan. Sobreviven: el filtro de re-colocación
(equivalente: es el código muerto de arriba); los dos umbrales de isla chica
(equivalentes con malla: el router V3 siempre deja juntas de ruta y ninguna de
construcción, y entonces la bandera no cambia nada; invertirla entera tampoco
mueve el fixture); el mínimo vertical de 1,5 mm de la hoja de merge (casi
inalcanzable: pide dos contactos a menos de 1,5 mm, que el dedup y el filtro de
3 mm eliminan) y el tope infinito del tier ancho del fan de cavidad (ningún fan
de cavidad del fixture necesitó un tramo mayor que el tope derivado).

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 73 tests más
1 de escala ignorado.

## Fase 1, bloque 7 — auto-bracing (hecho)

| Archivo DF | Rust | Estado |
|---|---|---|
| `autoBracing/autoBrace.ts` (`buildAutoBracedSnapshot`) | `auto_bracing/auto_brace.rs` | completo: muestras de fustes (trunk, branch, kickstand), partición, pares por modelo (árbol de expansión mínima más un segundo eje por soporte), kickstands generativos, aristas de kickstand, escalera por niveles o zigzag y la reconciliación que suelta kickstands redundantes; `runAutoBracing` y `clearBracesForModel` (store e historial) son de la GUI |
| `autoBracing/voronoiPartitioning.ts` | `auto_bracing/voronoi_partitioning.rs` | completo (semillas con jitter de V8 `Math.sin`, FNV-1a, reclamo BFS); los marcadores de depuración son de la GUI |
| `autoBracing/{initialPattern,repeatingPattern,zigzagChain}.ts` | `auto_bracing/patterns.rs` | completo |
| `autoBracing/twoAxisDetection.ts` | `auto_bracing/two_axis_detection.rs` | completo |
| `autoBracing/{meshClearance,meshGeometryStore}.ts` | `auto_bracing/mesh_clearance.rs` | completo: nueve rayos doble cara; las mallas registradas entran como parámetro |
| `SupportTypes/Kickstand/kickstandStabiliser.ts` | `support_types/kickstand/kickstand_stabiliser.rs` | completo: dos pasadas, grilla cardinal, holguras de raíz y fuste, colisión inclinada por punto más cercano del BVH |
| `localeCompare` de V8 (ICU `en-US`) | `utils/collation.rs` | ASCII imprimible exacto (puntuación < dígitos < letras, mayúscula detrás en empate) |

`computeAutoSupportPlan` ahora corre el auto-bracing donde DF lo corre (paso 5, salvo `debugSkipAutoBracing`); el plan recibe las mallas de holgura y el store comprometido.

**Semántica DF reproducida:** el tipo de una entidad sin `typeId` se resuelve por el store comprometido (`getSupportTypeOf`). `buildKickstandData` no estampa `typeId`, así que un kickstand generado en una pasada no tiene muestras ni brazos en esa pasada. Además, los kickstands auto-generados se regeneran siempre y sus raíces y knots quedan en el snapshot. Están anotados como mejoras en SC-76.

**Evidencia:** `scripts/df-oracle/golden_autobrace.ts` → `tests/fixtures/df_autobrace.json`: 90 corridas sobre las siete mallas de 6c (seis registradas para holgura). Cada corrida planifica un bosque con islas sintéticas (la mitad con el bracing dentro del plan) y lo braza dos veces seguidas como `runAutoBracing`: clon del store, brazos, commit. Antes de la segunda pasada, parte de los kickstands y brazos pasa a ser del usuario. Los ajustes de bracing son aleatorios (patrones cruz, simple y zigzag en las nueve combinaciones; grilla on/off; diámetro, distancias, semillas, largo máximo; quitar o conservar brazos previos). Resultado:
- 1.093 brazos entre fustes y 62 a kickstands;
- 98 brazos del usuario conservados;
- kickstands generados y regenerados;
- 160 particiones de Voronoi directas, con alcance NaN y planos agrupados;
- 400 palabras para la colación.

`tests/df_parity_autobrace.rs`: **igualdad exacta** en todo. Sensibilidad: 19 de 24 mutaciones fallan. Sobreviven cinco:
- la fórmula del puntaje de eje (equivalente: para separaciones de 45° a 90°, `90 − |90 − sep|` es `sep`);
- el borrado de la reconciliación (casi inalcanzable: pide ejes reales distintos de los preliminares, solo con trunks inclinados);
- el tope de dos aristas por trunk de kickstand, las muestras de la colisión inclinada y el épsilon del origen de rayo (umbrales que el fixture casi no toca).

`cargo clippy -p support-gen-df --all-targets` y `cargo fmt` limpios; 77 tests más
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
