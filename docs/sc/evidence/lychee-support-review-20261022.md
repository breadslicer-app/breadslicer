# Revision del informe de soportes Lychee 7.5.2

Fuente local: C:/Users/dnaon/AppData/Local/Temp/lychee-inspect-ssif_bn8
Revision: 2026-10-22. Solo lectura del material original; sin abrir Lychee ni ejecutar sus bundles/addons. SC y DF no modificados.

## Dictamen

Muy buen mapa tecnico consolidado y catalogo de configuraciones guardadas. Suficiente para dejar de especular sobre la separacion de responsabilidades del sistema y para definir nuestros propios datos de soportes. No equivale a recuperar un motor completo ejecutable ni a demostrar que otra implementacion producira identicos soportes.

El informe final y la matriz prevalecen sobre checkpoints anteriores. Por ejemplo, SUPPORT-STRUCTURE-FINDINGS conserva una antigua duda sobre el caller de auto-bracing que la consolidacion posterior declara trazada. No confundir esas notas historicas con pendientes actuales.

## Cobertura documentada

- Ruta moderna: secciones, continuidad entre capas, clases de islas, microislas, metricas de area/volumen, candidatos y distribucion.
- Ruta legacy seleccionable: imagenes, componentes por pixeles, influencia de contactos previos, demanda residual, muestras y relajacion.
- Construccion comun: salida del contacto, bajada vertical, recorridos inclinados, minis, puntas, cuerpos, bases y bases sobre objeto.
- Parenting y bracing opcionales, grillas, patrones y cathedral.
- Visualizacion instanciada y actualizaciones por cambios, separadas de la geometria de salida.
- Exportacion STL y rutas de seccion/raster CPU/GPU.
- Vine separado del camino moderno habitual; su mera presencia en el binario no prueba que la UI lo utilice.

Estos puntos son resultados del informe estatico revisado, no una reconstruccion independiente de cada funcion ensamblada durante esta revision.

## Catalogo efectivo del snapshot

| Categoria | Inventario |
|---|---|
| Geometrias SLA de fabrica | Light, Medium, Heavy |
| Geometrias SLA custom | 4 J3D: Small, Medium, Large, Exra-Small |
| Packs autosupport guardados | Default; Figure - 75mm +; MIni - 35mm - Tall |
| Packs custom | Los dos ultimos, no seis |
| Bracing guardado | 6 presets, con IDs separados aunque dos comparten nombre |
| Historico documentado | 42 backups locales y 15 variantes de configuracion, separados del estado actual |

Contactos J3D: Small0.24mm, Medium0.30mm, Large0.40mm, Exra-Small0.18mm. Punta principal: diametro1.1mm, largo2.3mm. Pie: diametro5mm, espesor0.25mm. Esto describe medidas guardadas, no resistencia ni el resultado final de todos los recorridos.

## Comprobacion independiente de esta revision

32/32 controles aprobados, detalle en lychee-support-review-20261022.json:

-13 hashes de informes/entregables contra sus manifiestos.
-475 campos de geometria,311 de autosupport y102 de bracing:888 valores CSV coinciden con el catalogo JSON.
-30 referencias geometria/pack resuelven y coinciden con sus rutas de datos.
-Las4 geometrias custom y los3 packs coinciden con los JSON del snapshot sin consolidar.
-6 bracings guardados confirmados.
-2 extractos de defaults coinciden exactamente con los offsets Unicode del bundle;8 fuentes de enums coinciden con sus hashes.
-Identidad SHA256 de los2 binarios nativos moderna/legacy coincide con el informe.

No se volvio a ejecutar la suite original. Sus resultados registrados incluyen215 controles de presets; eso no equivale a215 pruebas funcionales del algoritmo.

## Detalles que no hay que simplificar

1. Default/asp0 refiere bp0, que no esta en los seis bracings guardados. El informe documenta un fallback UI, no un bracing recuperado ni una garantia universal.
2. J3D Medium/sp2 guarda base.joinLength22.28817055514496 y newJoinLength1.07. Ambos se preservan; no decidir por intuicion que uno es el correcto en todas las rutas.
3. Figure75mm+ guarda overhang.preset=supportMedium (fabrica, contacto0.6mm). No es automaticamente J3D Medium0.3mm. Tambien guarda subpresets de overhang, pero el informe no demuestra su consumo efectivo por las rutas inspeccionadas.
4. densityValue es separacion con unidad de longitud; no cantidad por area. secondary.height no es altura total de pilar. tip.angle100 no debe interpretarse simplemente como inclinacion100grados.
5. Campos extra y algunos campos de packs pueden ser residuales/no consumidos en las rutas revisadas. Preservarlos como datos no certifica su efecto.
6. Colisiones por rayos discretos y heuristicas de fuerza no son pruebas exactas de volumen ni FEA. STL concatenado no demuestra union manifold. Analisis estatico no valida impresion, estabilidad mecanica o equivalencia CPU/GPU.

## Lectura util para SC

La separacion mas aprovechable es: contactos -> recorridos -> geometria -> parentesco/refuerzos -> representacion/salida. No construir un bloque monolitico de autosupport.

Primer objetivo razonable: modelo parametrico propio, soportes manuales editables y geometria consistente de punta/cuerpo/base; usar las medidas personales como referencias comprobables. Luego minis, bracing y parenting; finalmente generacion automatica de contactos, con pruebas independientes. Los presets solos no aportan el algoritmo de ubicacion ni aseguran la misma calidad de Lychee.

El expediente contiene codigo/ensamblado derivado de propietario: es material de investigacion, no un modulo listo para copiar a SC ni una certificacion clean-room. Esta revision no propone integrar esos binarios o fragmentos.

## Entradas recomendadas

- FINAL-STATIC-REPORT.md: resumen consolidado y reconciliacion de hallazgos.
- SLA-SUPPORT-EVIDENCE-MATRIX.md: confirmado/inferido/limites y referencias.
- presets/MEDIDAS-Y-PACKS.md: lectura humana de dimensiones y packs.
- presets/SUPPORT-PRESET-CATALOG.json: datos completos sin redondear.

Los archivos fuente permanecen en Temp; conservarlos fuera de Temp seria conveniente antes de usarlos como referencia de largo plazo. Esta revision solo guarda sus propios resultados en el workspace.
