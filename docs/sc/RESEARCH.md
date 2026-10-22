# Referencias de investigación — no implementación

## Soportes Lychee

Revisión propia en [evidence/lychee-support-review-20261022.md](evidence/lychee-support-review-20261022.md).
Entrada original: `FINAL-STATIC-REPORT.md`; presets:
`presets/MEDIDAS-Y-PACKS.md` y `SUPPORT-PRESET-CATALOG.json`.
3 geometrías de fábrica, 4 J3D, 3 packs guardados (2 custom), 6 bracings.
32 controles independientes registrados, no reejecutados para el inventario.

Separar contactos → rutas → geometría → parenting/bracing; preset geométrico,
pack de colocación y bracing son entidades distintas. Mantener rarezas de datos
como tales, no convertirlas automáticamente en reglas de SC.

## Hollow Lychee

Entrada original: `hollow/FINAL-HOLLOW-REPORT.md`, parámetros en
`hollow/PARAMETERS.md`; límites en `hollow/CLOSURE-AUDIT.md`; registro en
`hollow/FINAL-VERIFICATION.json`. Revisión previa: 336 hashes/tamaños cotejados;
el registro declara 1504 controles estáticos con solapamientos + 81 de auditoría.
No se reejecutaron suites ni se compararon resultados runtime en esta tarea.

Implicaciones para requisitos propios:

- Hollow 2D/raster y hollow 3D/malla son capacidades diferentes.
- Espesor físico debe tener política explícita ante escala no uniforme.
- Blockers, drenajes, tapones y su historial deben persistirse como parámetros.
- No reducir silenciosamente el diámetro solicitado; reportar nominal/efectivo.
- Preview, bake, exportación y slices deben contrastarse entre sí.
- Validez de caché por contenido/revisión, no sólo timestamps de interacción.
- Detectar cavidades sin drenaje, volúmenes atrapados y riesgos de ventosa antes
  de presentar una pieza como lista para imprimir.

## Ubicación y conservación

Originales: `C:/Users/dnaon/AppData/Local/Temp/lychee-inspect-ssif_bn8/`.
Respaldo local durable (fuera del repositorio):
`research/lychee-static-20261022.zip` y `.manifest.json` en el workspace.
No depende de que Temp sobreviva. Se conserva como investigación privada, no
como código de SC. Los bundles/extractos de terceros no se suben al repo.

Para implementar se hará un preflight de opciones abiertas mantenidas, licencias,
integración Rust/FFI, robustez y rendimiento. Estas referencias no prueban
equivalencia funcional, clean-room, estanqueidad universal ni comportamiento impreso.
Lychee continúa como herramienta de producción; DF sigue preservado.
