# SC-08 — baseline y recuperación

Fecha de inventario: 2026-10-22. Código funcional: `22526a29` (sin cambios de
comportamiento durante SC-08/09). La build entregada contiene navegación, gizmo
combinado, contacto real con cama y transformaciones modales.

## Remotos y ramas

| Referencia | Uso |
|---|---|
| `origin` → `https://codeberg.org/SoulCrafted/Slicer.git` | Upstream original, sin escrituras |
| `github` → `https://github.com/SynrgStudio/SC-private.git` | Nuestro respaldo privado |
| `main` / `github/upstream/main` | Baseline upstream `fd238bbd` |
| `sc/development` | Nuestra continuación, incluida documentación |
| `experiment/selection-gizmo` | `93742b0e` |
| `experiment/bed-clearance` | `69427db4`, solución intermedia conservada |
| `experiment/tight-bed-contact` | `78db33d1` |
| `experiment/modal-transforms` | `6a4e0427` |
| `experiment/blender-navigation` | `22526a29` |
| `sc-editor-verified-20261022` | Retorno inmutable al código aprobado |

Todas estas ramas están respaldadas; no se fusionaron experimentos ajenos ni se
modificaron DragonFruit, Lychee o el checkout de revisión.

## Ejecutar sin instalar

Usar la carpeta portable original `output/SoulCrafted-navigation-hover-fixed/`
del workspace, o descomprimir el asset de la release privada en una carpeta nueva.
Conservar `slicer/` (perfiles), licencias y ejecutable juntos. No registrar
asociaciones ni sustituir el slicer de producción.

SHA256 del EXE: `ebcc86255426599ed33d08f50bc5aa6650bb5cf91d70944962aecb44db5f01a2`.
Tamaño: 41,607,680 bytes. El hash identifica el artefacto aprobado; no prueba
reproducibilidad binaria de una recompilación.

## Compilar

En una copia nueva o un worktree sin cambios locales:

```powershell
git clone --branch sc/development https://github.com/SynrgStudio/SC-private.git SC
cd SC
cargo build --locked --release -p sc-slicer-gui
```

El checkout original compiló con Rust/Cargo 1.97.1 y herramientas MSVC de Windows.
`workspace.package.rust-version = 1.98.1` no está heredado por los paquetes;
no instalar otra toolchain sólo por esa declaración. El build necesita acceso a
dependencias no cacheadas. Revisar `packages/windows/build.ps1` para staging y
`assets/profiles/` para perfiles. El EXE de `target/release` aislado no es el
paquete portable completo. No se recompiló para este inventario documental.

## Volver al código aprobado sin destruir trabajo

```powershell
git fetch github --tags
git worktree add ../SC-verified sc-editor-verified-20261022
```

Ese checkout separado permite compilar o inspeccionar el baseline sin resetear la
rama de desarrollo. Para usar inmediatamente la build aprobada, descargar el asset
de la release y comprobar su SHA256; no hace falta recompilar.

## Evidencia registrada, no reejecutada en SC-08

- Core/escena/sliceado seleccionados: 898 pruebas + 57 integración espacial.
- Contacto con cama: 9 controles y 62 contrastes geométricos.
- Modal: 8 matemáticos y 20 controles nativos.
- Cámara: 22 pruebas del módulo.
- Radial corregido: 17 controles nativos en el EXE de este hash, más aprobación
  explícita de Damian. La entrega anterior `fe661cde` falló el hover y NO es baseline.
- No se afirma suite completa GUI, paridad con Lychee ni impresión validada.

Los registros relevantes están en `docs/sc/evidence/`; los originales completos,
capturas y scripts están en `output/soulcrafted-*` del workspace. La release
respalda el paquete portable y evidencia seleccionada, no los modelos personales.
