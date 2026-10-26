# Port nativo del hollow de DragonFruit → SC

Rama `claude/hollow-engine-b6b5b3` (2026-10-26), tareas SC-18 y SC-39…SC-46 (la parte
del motor). Esta rama no toca la GUI: la pestaña Hollow del rediseño (SC-84) lo
conecta después, por `sc_geom::df_hollow`.

## Qué se portó

Fuente: DragonFruit `rust/dragonfruit-mesh-repair/src/hollowing.rs` en la revisión
privada `f331fc55`, que ya trae los aportes de Damian (SynrgStudio, 2026-10-21/22):
voxelizado por losas Z en paralelo (~5×), preview de superficie separada del Apply
(`preview_surface_only`), y grilla local al modelo (`gridSpace: 'local'` en el front
de DF, que en Rust es rotación identidad). De `dragonfruit-mesh-core` se portó lo que
el motor usa: `IndexedMesh`/`Vec3`/`Aabb`, el BVH plano y la topología de aristas;
de `dragonfruit-mesh-repair`, el suavizado de Taubin.

| Pieza | Dónde | Estado |
|---|---|---|
| Motor DF (`hollow_voxel`, `HollowSession`, cavidad suavizada, chaflán, infill lattice/pillar, áreas sólidas por índice de vóxel, lazo de selección, corredores de drenaje por vóxel) | `libs/sc-geom/hollow-df/src/hollowing.rs` | Portado con la misma aritmética `f32`: paridad bit a bit |
| Estabilización del Apply (`finalize_hollow_output_mesh_for_manifold`) | ídem, `finalize_hollow_output_mesh` | Misma escalera (soldado, menos suavizado, sin suavizado); el round trip de manifold3d se reemplaza por el chequeo edge-manifold del slicer de SC |
| Drenajes (`punch_cylinders`) | `src/drain.rs`, `src/punch.rs` | Corte exacto propio en Rust (ver abajo); misma especificación y número de segmentos que DF |
| API de SC | `src/engine.rs` (`Session::build`, `preview`, `apply`) | Nueva |
| Adaptador de escena | `libs/sc-geom/hollow-df-sc` (`start_hollow` → `JobHandle`), reexportado como `sc_geom::df_hollow` | Nuevo |

No se portó: `ShellOpenFace` como modo de SC (está en el motor por paridad, pero la UI
de DF lo convierte en `cavity` y su salida es abierta), el túnel de vóxeles que DF usa
como fallback de drenaje sin manifold (DF mismo lo rechaza en su build), ni la
orquestación TS de DF (colas de IPC, buffers, WebView): en SC la hace el job.

## Licencia

`dragonfruit-mesh-repair` y `dragonfruit-mesh-core` son **MIT** (Copyright 2026 Open
Resin Alliance; el `LICENSE` del crate, distinto del AGPL de la raíz de DF). `hollow-df`
queda MIT con ese aviso (`COPYING`, `NOTICE`); el código nuevo de SynrgStudio en ese
crate también es MIT, para poder devolverlo a DF si Damian quiere. MIT es compatible
con el AGPL-3.0 de SC. El adaptador `hollow-df-sc` depende de crates AGPL de SC y es
AGPL-3.0-or-later, como `support-gen-df-sc`. No se copió nada de Lychee.

## API (`hollow-df`)

- `Session::build(mesh, &GridRequest { voxel_size_mm, max_resolution }, max_cells, ctl)`:
  voxeliza una vez. La malla debe estar en un marco cuyos milímetros sean físicos.
  `GridInfo` informa el vóxel pedido y el **usado**, la resolución, si el límite lo cambió
  (`ResolutionBound::Coarsened`/`Refined`), las dimensiones y un estimado de memoria.
  DF recorta la resolución a 24..=192 en silencio; acá el máximo es un ajuste (tope 512)
  y `max_cells` rechaza grillas gigantes antes de reservar memoria.
- `Session::preview(&HollowSettings, ctl)`: la superficie de cavidad hacia adentro, del
  mismo `keep` que el Apply, sin suavizado ni estabilización (la preview de Damian).
  Más los centros e índices de vóxeles removidos en la pared, para editar áreas sólidas.
- `Session::apply(&settings, &[DrainHole], ctl)`: la malla imprimible. Cáscara original +
  pared de cavidad suavizada (+ infill), comprobada edge-manifold con la escalera de DF,
  y los drenajes cortados. Si no se puede cerrar, **error explícito**
  (`HollowError::NotClosed` / `Drain { index, error }`), nunca una malla rota.
- `HollowSettings`: relleno (`Cavity` o `Infill { Lattice | Pillar, cell_mm, beam_radius_mm }`),
  pared, suavizado/chaflán, áreas sólidas (`SolidArea::Voxels` de DF, o `Sphere`/`Box` en mm,
  que sobreviven a un cambio de vóxel).
- `DrainHole { center, direction (hacia adentro), radius_mm, radius_y_mm, depth: Auto | Mm, outside_mm }`.
  `Auto` es la profundidad de DF: pared a lo largo del eje + 1 mm (tope 120), medida por
  rayo; si no encuentra pared usa pared + 1 y lo marca (`depth_fallback`). Cada drenaje
  informa si llega a la cavidad (`reaches_cavity`).
- `HollowStats`: vóxeles ocupados/removidos, volumen removido por vóxeles, volumen de la
  fuente, de la cavidad y (en Apply) de la salida y lo ahorrado, en mm³ (÷1000 = ml),
  medidos sobre las mallas; reintentos de suavizado.
- `Control`: cancelación (token propio o predicado de otro sistema de jobs) y progreso por
  etapa (`Stage`: Voxelize, FloodFill, ParityRefine, DistanceField, KeepMask, CavitySurface,
  Infill, Stabilize, Drains, Validate). Los bucles largos consultan la cancelación.

## Adaptador (`hollow-df-sc`)

`start_hollow(scene, leaf, HollowRequest)` corre en un hilo y devuelve `JobHandle<HollowOutcome>`.
El transform del leaf `M` se separa por descomposición polar en rotación/espejo `R` y
estiramiento simétrico `P` (`M = R·P`); el motor trabaja sobre `P·malla`.

- **Pared física bajo escala no uniforme:** DF convierte la pared con el promedio de los
  ejes de escala (una esfera estirada 2×1×1 recibe 1,5 mm en Y/Z y 3 mm en X para "2 mm");
  acá queda 2 mm en todos los ejes (test `the_wall_is_physical_under_a_non_uniform_scale`).
- **Grilla local:** mover o rotar el modelo sólo cambia `R` y la traslación, así que la
  sesión se reutiliza (`HollowRequest::prepared`, `HollowOutcome::reused_session`); un
  cambio de escala, de malla o de vóxel la reconstruye. La clave incluye un hash del
  contenido de la malla.
- Los resultados vuelven en el **marco local** del leaf (`P⁻¹`), para mostrarlos con el
  transform del leaf o instalar la malla del Apply en lugar de la original (que se conserva
  para deshacer). Drenajes y esferas sólidas se dan en coordenadas de mundo de la pose
  actual; `LeafFrame` convierte para quien los persista en el modelo (SC-43).

## Drenajes: corte exacto propio

DF corta con manifold3d (C++, su build clona y compila con CMake); en este entorno no hay
CMake ni red, y agregar un toolchain C++ al build de SC es una decisión aparte. Se escribió
el caso que hace falta: malla cerrada edge-manifold menos un **prisma convexo** (el cilindro
de DF con sus mismos segmentos, `ceil(2πr/0,7)` entre 16 y 80).

Cada vértice nuevo tiene un nombre simbólico calculado una sola vez — vértice de la malla,
arista×plano del cortador (interpolada sobre la arista canónica), triángulo×arista del
cortador, esquina del cortador — así los dos triángulos de una arista reciben exactamente
el mismo punto y el resultado es edge-manifold por construcción. Cada triángulo que toca el
cortador se recorta con Sutherland–Hodgman sobre puntos con nombre; la parte de afuera se
retriangula; las aristas recortadas que caen en un plano del cortador forman las curvas de
corte, que en cada cara del cortador delimitan la pared del agujero. De qué lado está cada
esquina se lee de las normales en los cruces; sólo un cortador cuyas aristas no cruzan nada
pregunta con un rayo. Un vértice está "sobre" un plano sólo por nombre: los empates
numéricos caen siempre del mismo lado. Si el redondeo igual rompe el resultado, el cortador
se gira y agranda 1e-5 y se reintenta (4 intentos); si no, error.

Verificado con volúmenes exactos (agujero pasante, ciego, oblicuo/ovalado en esfera,
alineado con la grilla de la malla, hueco con drenaje que une cavidad y exterior en una sola
shell) y con `punch_cylinders` estilo DF. Límite: supone malla sin autointersecciones cerca
del agujero; con infill, un drenaje que cruce una viga embebida en la pared puede fallar, y
falla con error.

## Paridad con DF

`scripts/df-oracle/run_hollow_harness.sh` compila el crate Rust de DF (features por defecto,
sin manifold) desde su checkout sin modificarlo y escribe
`libs/sc-geom/hollow-df/tests/fixtures/df_hollow.json`: 11 casos (esfera preview/final por
sesión y por `hollow_voxel`, cavidad sola, lattice con chaflán, pillar preview, cajas
anidadas, rotación, vóxeles bloqueados, corredor de drenaje por vóxel, pared gruesa con
chaflán). `tests/df_parity_hollow.rs` arma los mismos casos y compara informes, índices y
centros de vóxeles, y hashes de posiciones e índices de cada malla: **iguales bit a bit**.

Con `SC_BENCH_STL` el harness escribe además `.local/df-oracle/df_hollow_real.json` (modelo
privado, no versionado); el test ignorado `matches_dragonfruit_on_the_real_model` también da
**igual bit a bit** en `617_repaired.stl` (1 889 470 triángulos, 90 mm, resolución 171).

## Medición (Ryzen 7 5800X, release)

`617_repaired.stl` escalado a 90 mm de alto, vóxel 0,65 mm (usado 0,6485, 171 vóxeles en el
lado mayor, grilla 164×173×141, ~91 MB), pared 2 mm:

| Paso | SC | DF (su crate Rust, misma máquina) |
|---|---:|---:|
| Sesión (voxelizar, inundar, paridad, distancias) | 857–910 ms | 1 015 ms |
| Preview (superficie de cavidad, 339 920 triángulos) | 236–254 ms | 263 ms |
| Apply sin drenaje (incluye chequeo edge-manifold) | 2,1–2,6 s | 2,3 s sin estabilizar; su Apply real suma el round trip de manifold3d |
| Apply + 1 drenaje automático (r 1,5 mm) | 2,7 s | — |

Fuente 190,50 ml → hueco 119,33 ml (71,17 ml ahorrados; la estimación por vóxeles da 71,89).
Salida edge-manifold, sin reintentos de suavizado; con el drenaje bajo la cavidad (profundidad
automática 7,0 mm) cavidad y exterior quedan en una sola shell. Comando:

```text
SC_BENCH_STL=C:/Users/dnaon/Downloads/617_repaired.stl \
  cargo test --release -p hollow-df --test bench_real_model -- --ignored --nocapture
```

El corte de drenaje sobre 2,1 M triángulos bajó de 2,25 s a 0,15 s al pasar el chequeo
edge-manifold a un ordenamiento paralelo (0,65 → 0,06 s) y leer el lado de las esquinas de
los cruces en vez de construir un BVH.

## Diferencias con DF y por qué

- **Vóxel efectivo visible** y límite de resolución configurable (DF: clamp 24..192 silencioso).
- **Pared física** con escala no uniforme (DF: promedio de escala).
- **Drenajes exactos en Rust** en vez de manifold3d; error explícito por drenaje.
- **Profundidad automática medida sobre el hollow final.** DF la mide sobre la geometría de
  preview que esté mostrando; desde la preview de sólo superficie, esa geometría es la
  cavidad sola, y "primer y segundo impacto" pasan a ser las dos paredes de la cavidad, no
  la pared del modelo. Anotado en SC-76.
- **Volúmenes en mm³ medidos sobre las mallas** y cancelación/progreso por etapa.
- **Áreas sólidas en mm** además de índices de vóxel.

## Hallazgo para la integración: el slicer y la cavidad sin drenaje

Un hollow sin drenaje tiene la cavidad como **shell aparte** (hacia adentro). `sl1cer` agrupa
contornos por shell (SC-81), normaliza cada anillo a CCW y agrupa por contención **dentro de
cada shell**, así que lee la cavidad como un segundo sólido: a media altura de una esfera
hueca da 2 polígonos y 0 agujeros (test ignorado `an_undrained_hollow_slices_with_a_hole` en
`hollow-df-sc`). Con un drenaje que llega a la cavidad, cavidad y exterior son una shell y se
rebana bien (1 polígono, 1 agujero). Arreglo propuesto, en `sl1cer`: tratar una shell de
volumen con signo negativo como vacío, es decir, sus anillos como agujeros de la shell que la
contiene. Hasta entonces la GUI debería exigir un drenaje (o advertir) antes de rebanar un
hollow.

**Resuelto (2026-10-26):** `sl1cer` calcula el volumen con signo de cada shell (`FaceShells`).
Los anillos de una shell vacía restan del polígono sólido más chico que los encierra
(`subtract_voids` en `api.rs`). El test ignorado vuelve a correr. La GUI igual avisa si la
cavidad queda sellada, porque es un problema de impresión: resina atrapada y efecto ventosa.

## Tests

- `hollow-df`: 43 (20 portados de DF, 3 propios del motor, corte de drenajes,
  API de SC, cancelación, progreso por etapa), paridad DF (11 casos), modelo real (ignorado).
- `hollow-df-sc`: 6 (+1 ignorado): descomposición polar, reuso de sesión al mover/rotar y
  reconstrucción al escalar, pared física con escala no uniforme, Apply con drenaje rebanado
  por `sl1cer`, cancelación.
- Clippy sin avisos en ambos crates.
