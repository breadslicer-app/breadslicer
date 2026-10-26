# Cómo trabajamos: ideas, issues y microPRs

Desde el 2026-10-26 el tracker operativo son los **issues de [breadslicer-app/breadslicer](https://github.com/breadslicer-app/breadslicer/issues)**:
- [`ROADMAP.md`](ROADMAP.md) queda como contexto, decisiones e historial;
- [`ISSUES.md`](ISSUES.md) traduce cada SC-n a su número de issue.

## Ideas nuevas

1. Damian deja las ideas en una lista, tal como le salen: en un issue con la etiqueta `idea` o en un archivo.
2. La lista se ordena:
   - los duplicados se unen con issues existentes;
   - las ideas grandes se separan en partes chicas;
   - cada una lleva su "qué" y su "cuándo está terminado".
3. Cada una se sube como issue con su área (`area: …`) y estado (`state: backlog`), y se enlazan sus dependencias ("Depende de #n").
4. Lo que es decisión de Damian (prioridad, diseño, comportamiento visible) se pregunta. No se inventa.

## microPRs

- **Tamaño:** un PR resuelve un issue o una parte chica de uno. Mejor diez PRs chicos que uno gigante.
- **Rama:** `issue/<n>-<slug>` (por ejemplo `issue/3-soporte-sin-prisma`), desde `main`.
- **Contenido del PR:** qué cambió y por qué; cómo se probó (tests que corrieron; qué falta probar a mano); y `Closes #n`, o `Part of #n` si es una parte.
- **Merge:** lo hace Damian. Hasta que haya CI propio, el PR dice qué tests corrieron.
- **Reglas del repo que se mantienen:**
  - nada se imprime ni se publica sin autorización;
  - cada build nuevo va en su propia carpeta;
  - nunca se debilitan las protecciones de datos.

## Etiquetas

- **`area: …`:** soportes, hollow, slicing, editor, etc.
- **`state: …`:** `in progress`, `todo`, `backlog`, `triage`, `paused`.
- **`roadmap`:** vino de la migración del roadmap.
- **`idea`:** ideas sin ordenar todavía.
- **`microPR`:** chico y listo para tomar.
