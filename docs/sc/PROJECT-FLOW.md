# SC-14 — flujo real de proyecto e historial

## Alcance y método

Validación nativa Windows sobre copias sintéticas del corpus SC-24, con la build
aprobada `22526a29` como reproducción inicial. Los archivos guardados se contrastan
con un inspector ZIP/XML independiente: triángulos orientados normalizados,
geometría en coordenadas world, nombres, transformaciones y partes TOML de proyecto
y perfiles. No se toma el título o la captura como prueba de integridad del archivo.

## Resultado de la build inicial

14 comprobaciones de archivo aprobadas en 13 estados guardados:

- Guardar como conserva los tres objetos del proyecto de prueba, sus nombres,
  geometrías, poses y parámetros/perfiles embebidos.
- Copiar/pegar agrega un objeto; undo elimina exactamente ese cambio; redo
  restaura geometría y pose exactas.
- GX modifica la pose, no la forma; undo y redo restauran estados exactos.
- Borrar elimina un objeto; undo y redo mantienen los estados esperados.
- Guardar, abrir de nuevo y volver a guardar conservan los cuatro objetos.
- Un contenedor 3MF inválido informa el error y conserva el documento anterior.
- Guardar como → Cancelar conserva el estado. Los archivos originales del corpus
  permanecen intactos.

Estas comprobaciones no certifican todos los gestos, todas las reparaciones,
equivalencia raster/AA ni resultados impresos.

## Fallo reproducido y corrección

Después de borrar un objeto (documento dirty), **Archivo → Abrir** reemplazó el
documento sin confirmación. El archivo guardado previo seguía disponible, pero
se perdía el cambio no guardado y su historial de esa escena.

La ruta del selector nativo llamaba al loader directamente. La protección de
`AppState::open_path_in_window`, que ya atendía otras entradas, no participaba.
La corrección deriva las aperturas dirty a esa protección y libera antes el
`LoadTicket` del selector: Guardar y abrir necesita poder tomar un ticket nuevo.
Las aperturas limpias conservan su ruta, que mantiene la escena ante un error de
lectura. Cancelar un preview modal antes de abrir tampoco crea un edit guardado.

El examen del mismo flujo encontró dos salidas sin protección: **Cerrar escena**
y la **X de la ventana**. Ahora usan el mismo contrato de continuación: Cancelar
mantiene el documento; Descartar es explícito; Guardar/Exportar sólo cierran o
abren el siguiente documento tras un resultado exitoso y vigente. Un guardado
fallido o que quedó obsoleto no autoriza el descarte.

## Estado de validación de la corrección

La release nueva compiló en 4m14s. SHA256 del candidato:
`014dd54974ca222195a93600f6d76d0043704033defbb381db95cb2190b8fa7e`.
Se agregaron dos pruebas de ciclo de vida del ticket;
la primera compilación de tests encontró un import `Transform3D` faltante en los
tests antiguos del controlador (26 errores derivados de ese único import). Se
restauró bajo `cfg(test)`, sin cambiar ese controlador en producción.

Pasaron **126 tests focales**: 2 nuevos del ticket, 31 de guardado, 6 de política
de reemplazo, 42 de dirty/historial, 1 de cierre limpio, 19 de reparación y 25 de
exportación. Formato y diff-check pasan. No se ejecutó la suite completa de GUI.

Clippy estricto encontró **6 diagnósticos en código anterior**: `manual_clamp`
en cámara, `result_large_err` en exportación y `collapsible_if` en el controlador.
No se declaran resueltos por SC-14. Una segunda pasada permitió exclusivamente
esas tres categorías mediante flags del comando y pasó las restantes; eso **no**
equivale a Clippy estricto aprobado. No se agregaron supresiones al código.

La release corregida pasó **9 comprobaciones nativas adicionales**, con permiso
explícito de Damian para usar el escritorio:

- Dos STL cargados en ventanas separadas se copian al proyecto: seis objetos,
  formas cotejadas con el corpus y perfiles/TOML originales conservados.
- Abrir dirty → Cancelar conserva modificaciones y geometría.
- Cerrar ventana → Cancelar conserva ventana y cambios.
- Cerrar escena → Cancelar conserva la escena y los cambios.
- Guardar y abrir con destino read-only informa fallo, conserva bytes originales
  y mantiene la escena dirty. Al retirar read-only, la misma acción guarda los
  dos objetos actuales antes de abrir el STL solicitado.
- Cancelar el selector de Guardar como y abrir conserva el STL dirty.
- Descartar y abrir requiere elegir explícitamente el descarte.
- Reparar caras invertidas cambia el winding, conserva límites y permite undo
  que restaura exactamente la geometría previa.

El harness necesitó dos reanudaciones por sincronización del selector y por el
nombre de botón con puntos suspensivos en PowerShell; no fueron fallos nuevos
del programa. La evidencia registra los estados finales y el guardado intencional
que reemplazó el checkpoint de tres objetos por el estado de dos objetos.
Los 14 checks iniciales siguen atribuidos a la build inicial, no al candidato.
Exportación tiene 25 tests focales, no una matriz nativa de todos los formatos.
SC-14 queda cerrada en este alcance; la suite completa, autosave y robustez
multiformato ampliada pertenecen a tareas posteriores.

## Observaciones de uso y límites

- En este equipo `Ctrl+Shift+S` lo interceptó RSnip. Para Guardar como se usó el
  menú nativo; no se modificaron atajos globales.
- El menú desplegable es una ventana Win32 separada (`#32768`): una captura de
  la ventana principal no demuestra si el menú abrió o no.
- La lista de objetos virtualiza filas. Se verifica el número de objetos del
  archivo, no contando únicamente los elementos visibles de UI Automation.
- Pegar usa la posición del cursor y puede no actuar cuando el rayo no cruza
  la cama. Se prueba sobre la cama, sin asumir que una pulsación implica éxito.
- Los perfiles del corpus son sintéticos; se transportan embebidos y no se
  instalan ni se emplean para imprimir.
- Soportes paramétricos, hollow, autosave y validación física siguen en sus
  tareas propias. SC-14 no los declara implementados.
