# Rumbo y decisiones

Las tareas están en los [issues](https://github.com/breadslicer-app/breadslicer/issues), y el orden de trabajo vigente en el issue fijado "Orden de trabajo". Acá van solo las decisiones que guían el desarrollo.

## Decisiones (Damian, 2026-10-26)

- **La mejor experiencia de slicing que exista**, no "otro slicer más".
  - Todo tiene que sentirse fluido e inmediato.
  - Faltar impresoras o detalles es aceptable; una experiencia mediocre, no.
- **Ningún algoritmo heredado es intocable.** Lo que funciona bien se queda. Pero cuando hay una mejora real, se hace, aunque cambie el comportamiento previo.
- **Las reglas estructurales salen de la física, no de límites fijos.**
  - Ejemplo: un tope de miembros por tronco no tiene sentido si el tronco ya está arriostrado y anclado al modelo en capas anteriores.
  - La carga real la decide el análisis estructural (BS-11).
- **Análisis estructural:** fuerzas por capa más un modelo de vigas del árbol de soportes, en las capas críticas. La FEA 3D de tetraedros queda solo para detalles finos del modelo, y más adelante.
- **Calibración en dos niveles:**
  - un "chequeo rápido" de una placa;
  - una "calibración en serio" de 3–4 placas.

  Todas las probetas son propias, generadas por código (BS-78, BS-79). La lectura por foto queda para más adelante, con machine learning.
- **STL presoportados (BS-77):** se separan en cuerpos, y los soportes se reconocen y se pueden borrar. Poder "dejar solo este objeto".
- **Marca:** Breadslicer (`breadslicer.app`).
  - El cambio de nombre toca solo lo visible y migra los datos del usuario (BS-75).
  - Los nombres internos de código (`sc-*`) no cambian.
- **Licencias:** el proyecto es AGPL-3.0. Los avisos de licencia y autoría del código del que partimos se mantienen siempre, y el reconocimiento va al pie del README.
