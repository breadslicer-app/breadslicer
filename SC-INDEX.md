# SC — índice de nuestra versión

Este es el punto de entrada de desarrollo privado; el [README upstream](README.md)
se conserva sin reemplazar su autoría ni su documentación.

## Versión que funciona

- Código aprobado por Damian: `22526a29`, etiqueta `sc-editor-verified-20261022`.
- Desarrollo: `sc/development`; los cambios de inventario no modifican el programa.
- [GitHub privado](https://github.com/SynrgStudio/SC-private).
- [Build portable verificada](https://github.com/SynrgStudio/SC-private/releases/tag/sc-editor-verified-20261022).
- Ejecutable local: `../../output/SoulCrafted-navigation-hover-fixed/sc-slicer-gui.exe`.
- SHA256 del ejecutable: `ebcc86255426599ed33d08f50bc5aa6650bb5cf91d70944962aecb44db5f01a2`.

## Mapa

- [Corpus SC-24: casos, assets y reproducción](fixtures/sc-corpus/README.md).
- [Verificación del corpus](docs/sc/CORPUS-VERIFICATION.json).
- [Reconstrucción, ramas y retorno](docs/sc/BASELINE.md).
- [Matriz core → GUI → evidencia → trabajo pendiente](docs/sc/CORE-GUI-MATRIX.md).
- [Inventario reproducible de paquetes](docs/sc/INVENTORY.json).
- [Roadmap e instrucciones de mantenimiento](docs/sc/ROADMAP.md).
- [Roadmap estructurado](docs/sc/ROADMAP.json).
- [Snapshot del tablero con IDs y evidencia](docs/sc/BOARD-SNAPSHOT.json).
- [Verificación del inventario y respaldo](docs/sc/VERIFICATION.json).
- [Referencias de investigación](docs/sc/RESEARCH.md).
- [Tablero SC](https://oc.synrgs.tech/workboard/sc): estado operativo, no ejecución automática.

## Regla de mantenimiento

Al cerrar un cambio: actualizar la tarjeta, sus pruebas y artefactos; modificar la
matriz si cambia una capacidad; regenerar `INVENTORY.json` con
`python scripts/sc_inventory.py`; guardar en Git y hacer push al remoto `github`.
No marcar como probado lo observado sólo por lectura. Una entrega nueva no sustituye
la etiqueta aprobada hasta que tenga evidencia propia. El roadmap es revisable:
las ideas no son compromisos ni trabajos iniciados.
