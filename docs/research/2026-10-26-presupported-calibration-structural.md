# Investigaciones del 2026-10-26

Tres relevamientos que pidió Damian, hechos por agentes de solo lectura. Son propuestas para decidir. Cada uno tiene su issue: BS-77, BS-78 y BS-11.

## 1. STL presoportados: separar cuerpos en objetos (BS-77)

**Hoy:**
- Un STL se carga como **una sola hoja**. El lector suelda vértices solo por posición exacta (`indexed_mesh_from_soup`).
- `libs/sl1cer/src/shells.rs` ya separa shells por vértice compartido y calcula el volumen con signo (volumen negativo = cavidad sellada).
- `scene-edit` no tiene una edición para crear grupos.

**Cómo separar:**
1. **Conectividad:** union-find por vértice compartido, igual que `FaceShells`. Con la soldadura exacta alcanza para casi todos los exportadores; la soldadura con tolerancia queda como fallback.
2. **Cavidades:** los shells con volumen negativo se quedan con su contenedor. Nunca se vuelven objetos.
3. **Fragmentos:** los de menos de ~4 caras o volumen casi nulo se agrupan aparte. Nunca se borran en silencio.
4. **Unión booleana:** si el archivo se exportó así (un solo shell), no se puede separar. Hay que avisar, no adivinar.
5. **Dónde vive:** se extrae el algoritmo a `sc_mesh_core::shells::split_shells` y `sl1cer` pasa a usarlo.

**Clasificación** (solo una sugerencia para la UI; nunca borra sola):
- **Modelo:** el cuerpo de mayor volumen, más los que tengan ≥ 20 % de su volumen.
- **Raft:** minZ en la placa, bajo y con área XY grande.
- **Soporte:** cuerpo chico, alto y fino, que toca la placa o el raft y llega al modelo; suelen venir muchos parecidos.
- **Sin clasificar:** el resto, que decide el usuario.
- No hay documentación pública de cómo lo hacen Lychee o ChituBox. PrusaSlicer separa por shell sin clasificar.

**UX:**
- Al importar, si hay N > 1 cuerpos: "1 modelo + 312 soportes + raft (estimado). ¿Separar?". También queda el comando "Separar en cuerpos".
- **Resultado:** un grupo con Modelo, "Soportes importados" y Raft. Los soportes van en **una sola hoja con índice de cuerpo por cara**, porque miles de hojas castigan el picking y la GPU.
- **Edición realista:** seleccionar cuerpos de soporte (clic, caja o pincel) y borrarlos; "borrar todos los soportes importados"; generar soportes nuevos sin tratar los importados como modelo.
- **Más adelante:** regenerar soportes solo donde se borró, usando los puntos de contacto como semillas.

**Hito 1 (unos 2–3 días):** el comando "Separar en cuerpos", sin clasificación.
- Archivos: nuevo `libs/mesh-core/src/shells.rs`, `scene-edit` (separación como `Compound`), `scene_document.rs`, `wiring.rs` y el menú.
- **Aceptación:**
  - 2 cubos disjuntos dan 2 hojas;
  - un cubo con cavidad sigue siendo 1 hoja;
  - 2 cuerpos que se penetran sin compartir vértices dan 2 hojas;
  - undo y redo restauran exacto;
  - el slice es igual antes y después;
  - 1M de triángulos se separa en < 2 s.

## 2. El mejor calibrador de resina (BS-78)

**Estado del arte:**
- **UVtools:** Exposure Finder, Blooming, Elephant Foot, XYZ Accuracy, Tolerance.
- **Otros:** RERF, XP2 Validation Matrix, Cones of Calibration, AmeraLabs Town, la calibración de la Prusa SL1, DrLCD (mapa de uniformidad con un sensor) y la curva de trabajo de Jacobs (Cd = Dp·ln(E/Ec)).
- **Cómo se lee el resultado:** casi todo es visual ("elegí el mejor bloque"). **Ninguna herramienta lee resultados desde una foto.**
- **Física de la impresora:** nadie usa la irradiancia para convertir tiempo en dosis.
- **Dos objetivos distintos:** que la pieza mida lo que debe y que los soportes resistan piden exposiciones distintas.

**Propuesta: calibrador guiado de ~3 impresiones**, guardado por impresora + resina, con temperatura y lote:
1. **Curva de trabajo:** una placa con 8–12 ventanas a tiempos ×1,25. El usuario mide el espesor y el slicer ajusta Dp y Ec con R². De ahí sale la exposición recomendada para **cualquier** altura de capa.
2. **Detalle y resistencia:** postes y agujeros que se cuentan, conos o pilares de resistencia y ranuras, alrededor del tiempo predicho. La entrada es por conteo, no "elegir el más lindo".
3. **Base y pata de elefante:** variantes de erosión y dimming de base.
4. **Exactitud dimensional:** cajas en 9 zonas, con pin y agujero. Un ajuste lineal **separa escala de offset de pared**; UVtools solo calcula la escala. La diferencia entre zonas además detecta falta de uniformidad del LCD.

**Persistencia:** las mediciones crudas y el ajuste se guardan. Se aplica con undo y se avisa cuando la calibración queda obsoleta (otro FEP, ±5 °C, otro lote).

**Objetivo ambicioso:** leer los resultados por foto, con marcadores ArUco y detección binaria de postes y conos.

**Licencias:**
- UVtools es AGPL: reimplementar desde los conceptos, sin copiar sus clases.
- AmeraLabs Town, Cones y RERF no tienen licencia indicada: no empaquetarlos.
- **Todas las geometrías de prueba se generan por código, propias.**

**Hito 1:** placa de curva de trabajo + carga de espesores + ajuste Dp/Ec + recomendación por altura de capa + `CalibrationRecord`.
- El ajuste recupera datos sintéticos con error < 2 % y rechaza menos de 4 puntos o R² < 0,9.
- La validación física requiere la autorización de Damian.

## 3. Análisis estructural en el auto-support (BS-11)

**Fuerzas por capa:**
- **Peel del film:** es la que manda y es ~lineal con el área.
  - AmeraLabs con celda de carga: ~12 kPa con ACF y 13–19 kPa con FEP según el espesor. Una capa de 2000 mm² da ~30 N.
- **Succión** de copas sin drenaje: puede acercarse a ~100 kPa sobre su área. el motor de soportes ya calcula `cup_area_mm2`, pero no lo pasa a fuerza.
- **Arrastre viscoso:** se trata como multiplicador calibrado.
- **Gravedad:** sumergida casi no cuenta; afuera es ~1 N cada 100 g, y solo importa por palanca.

**Qué hacen los otros:**
- PrusaSlicer y PreForm dimensionan de forma empírica.
- Lychee detecta copas de succión.
- Hay papers con FEA de la separación y optimización topológica, pero **ningún generador publicado combina fuerza por capa con el dimensionado del árbol**. Es terreno poco cubierto.

**Enfoque recomendado**, de barato a caro:
- **(a) Presupuesto de fuerza por capa e isla:** demanda `σ_peel·A + p_suc·A_copa` contra capacidad `Σ A_punta·σ_punta` con factor de seguridad. Usa las áreas de `sl1cer` y las islas existentes; cuesta unos ms.
- **(b) Modelo de vigas del árbol:** troncos, ramas y riostras como barras. Se resuelve `K·u=f` disperso (faer o nalgebra-sparse) en las capas críticas, con pandeo de Euler. Reparte la carga según la rigidez real.
- **Descartados por ahora:** (c) FEA 3D con tetraedros, que es lenta y menos precisa que la incertidumbre del material, y (d) GPU, que no hace falta.
- **Dónde se engancha:** después de `compute_load_budget` en `auto_place/plan.rs`. La remediación sube de tier, agrega puntas en la isla deficitaria o avisa de succión. Va detrás de una opción, apagada por defecto hasta validarla.

**Material:**
- **σ_peel:** depende del film y el perfil de subida.
- **σ_punta:** resistencia en verde dentro de la resina; no está publicada y hay que medirla.
- **E en verde.**
- **Probeta:** una escalera de placas sostenidas por n puntas da directo `σ_punta/σ_peel` sin celda de carga. Se conecta con la calibración (BS-78 y BS-62).

**Hito 1:** (a) completo + (b) en las capas críticas, como reporte y remediación opcional. **Aceptación:**
- Determinista y < 5 % del tiempo del plan.
- Con la opción apagada, el resultado no cambia.
- Tests analíticos: viga, pandeo, reparto entre puntas.
- Sin colisiones ni huérfanos nuevos.
- El umbral predicho cae a ±1 escalón de la escalera física. Las impresiones requieren la autorización de Damian.

## Fuentes principales

- UVtools: https://github.com/sn4k3/UVtools
- AmeraLabs, peel por film: https://ameralabs.com/?p=45569
- Cones of Calibration: https://makers101.com/cones-of-calibration/
- Fauxhammer, exposición: https://www.fauxhammer.com/?p=35166
- DrLCD: https://github.com/yaqwsx/DrLCD
- Curva de trabajo (Liqcreate): https://www.liqcreate.com/?p=15658
- Yadegari et al., fuerzas de separación: https://journals.aut.ac.ir/article_4162.html
- Liravi et al. 2014, FEA con zona cohesiva: https://utw10945.utweb.utexas.edu/sites/default/files/2014-113-Liravi.pdf
- Prusa, Split to objects/parts: https://help.prusa3d.com/article/split-to-objects-parts_1751
- Lychee 7.6, Suction Cup Detector: https://lychee.co/changelogs/lychee-slicer-7-6-0
