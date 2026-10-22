# SC-24 — corpus sintético pequeño, versión 1

Material de desarrollo, **no piezas listas para imprimir**. Se reutilizan
`sc-mesh-fixtures` y `sc-scene-fixtures` del proyecto; no se descargaron modelos,
no hay escenas privadas y no se usa material extraído de Lychee.

## Entradas

Los archivos de [v1/](v1/) se pueden abrir manualmente en la GUI. El
[manifiesto](v1/MANIFEST.json) registra por caso procedencia, licencia, propósito,
unidades, expectativas analíticas y SHA256/tamaño de cada asset.
Los casos `*-core-only` son recetas Rust, no archivos de GUI: un importador 3MF
puede normalizar referencias en grupos y ocultar el límite que queremos probar.

| Familia | Casos / finalidad |
|---|---|
| Sólido y formatos | Cubo 10 mm en STL binario, STL ASCII (`.stla`), OBJ y 3MF |
| Unidades | Números 0..1: STL = 1 mm por convención; 3MF `inch` = 25.4 mm |
| Detalle / concavidad | Pared 0.2 mm y L extruida con área analítica 175 mm² |
| Huecos | Cavidad sellada con pared 2 mm; túneles cuadrados vertical/horizontal |
| Componentes / islas | Dos sólidos en una malla; dos objetos separados por un vacío Z10..15 |
| Transformaciones | Escala no uniforme, espejo y rotación Z90 preservados en escena |
| Estructura | Grupo Cube/ZTunnel/YTunnel; dos hojas comparten un mesh resource |
| Proyecto | 3MF con impresora/resina **sintéticas** embebidas y primera capa 0.05 mm |
| Geometría defectuosa | Cara ausente, winding inconsistente, caras colineales y cara duplicada no-manifold |
| Archivo corrupto | STL truncado, índice OBJ inválido, falso contenedor 3MF |
| Límite del core | Subtree Reference y CSG sin evaluar: rechazo explícito, no falso soporte |

Una cavidad sellada sirve para probar futura detección de resina atrapada; no
valida el generador hollow, que aún no existe como función de producto. La pared
fina no certifica imprimibilidad. Los perfiles embebidos no se instalan ni se
deben usar en una máquina real.

## Reproducir y verificar

Desde la raíz del repo, elegir una carpeta **nueva o vacía**:

```powershell
cargo run --offline --locked --release -p sc-project-formats --example sc_corpus -- ../../output/sc24-regenerated
python scripts/check_sc_corpus.py --compare ../../output/sc24-regenerated
```

El ejemplo usa los readers, writers, scene, project-formats y slicer reales del
core. Comprueba AABB, volumen, manifold/área de caras, payload de proyecto,
recurso compartido y cortes por planos con área/agujeros/polígonos conocidos.
Los rechazos de CSG/reference y validación geométrica comprueban su variante de
error. El script Python sólo comprueba integridad y comparación byte a byte;
**no sustituye** esas comprobaciones geométricas.

Sin regenerar: `python scripts/check_sc_corpus.py` coteja los hashes guardados.
El generador rechaza carpetas no vacías: no borra ni sustituye resultados
anteriores. `.gitattributes` impide que Git convierta finales de línea de assets.
La reproducibilidad byte a byte se comprueba con la toolchain y lockfile de esta
entrega; no se promete que futuras versiones de writers generen idénticos ZIPs.

## Qué está probado y qué queda para otros hitos

- **Core:** generación, lectura y expectativas indicadas en el manifiesto. Los
  valores esperados vienen de dimensiones/fórmulas de la receta, no de aprender
  la salida actual del motor y declararla correcta.
- **Cortes:** sondas analíticas, no comparación completa del raster ni AA.
- **Imágenes/GUI:** no se han generado golden screenshots ni automatizado gestos.
  En SC-14 usar cubo/grupo/proyecto para importar, transformar, copiar/borrar,
  undo/redo y guardar/reabrir. Comprobar geometría, nombres, poses y perfiles;
  no exigir IDs serializados idénticos ni jerarquía de wrapper idéntica.
- **Reparación:** sólo se reconoce el defecto y, donde corresponde, rechazo al
  sliceado. No se ha certificado que todas las reparaciones funcionen.
- **Impresión física:** ninguna. Ese cierre corresponde a SC-66/74 con Damian.
- **Rendimiento:** no hay modelos densos aquí. Worm/M03 y otros modelos de trabajo
  permanecen locales; sus mediciones pertenecen al corpus de rendimiento SC-54,
  no a los fixtures pequeños redistribuibles.

## Licencia y atribución

Apache-2.0, véase [LICENSE.md](LICENSE.md). Los builders originales reutilizados
pertenecen al proyecto SoulCrafted: copyright 2026 MyMiniFactory Ltd y CTRLback
SCCL; autoría conservada en los fuentes y el historial del repositorio. Las
recetas adicionales de este corpus y los assets sintéticos derivados se ofrecen
bajo la misma licencia. Los fixtures oficiales externos ya presentes en
`libs/mesh-formats/tests/fixtures/official/` conservan sus licencias y quedan
**fuera** de este paquete; pueden usarse como segunda línea de conformidad.

Para ampliar: añadir una receta con un resultado esperado independiente, generar
en carpeta nueva, verificar con core, revisar diferencias y después versionar.
No actualizar hashes para ocultar una regresión ni aceptar una salida sólo porque
es la nueva salida del programa.
