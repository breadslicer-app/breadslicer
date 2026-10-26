# Informe: UVtools (sn4k3/UVtools v7.1.1) y qué le sirve a Breadslicer

Relevamiento del 2026-10-26 (BS-80). Lo hizo un agente leyendo el código fuente de `UVtools.Core` y `UVtools.UI`. No reemplaza asesoramiento legal.

## 1. Licencia

- **UVtools es AGPL-3.0**, Copyright (C) 2020–2026 PTRTECH (Tiago Conceição). En los headers no aparece "or later", así que se trata como v3.
- **Portar o traducir C# a Rust está permitido**: es una obra derivada, y Breadslicer es AGPL-3.0.
- **Al portar hay que:**
  - mantener el copyright de PTRTECH en cada archivo portado ("Portions translated from UVtools…");
  - agregar una entrada en NOTICE o CREDITS;
  - marcar las modificaciones con fecha (§5a);
  - ofrecer el fuente (§13 si hubiera un servicio remoto);
  - no sumar restricciones.
- **Terceros:** no portamos nada de Emgu CV (GPLv3 o comercial; OpenCV debajo es Apache-2.0), ni de ImageSharp (Six Labors Split License), ni de las dependencias NuGet.
- **Formatos de archivo:**
  - Para CTB/CBDDLP/PHZ, la vía más limpia es **catibo** (Rust, BSD-2-Clause). uv3dp (Go, MIT) sirve como referencia.
  - **CTB cifrado: no se porta sin revisión legal.** Las claves están embebidas y el formato prohíbe la ingeniería inversa (riesgo de anti-elusión).
- **Conclusión:**
  - el código se puede reusar, con atribución;
  - algoritmos e ideas, libres;
  - geometrías de calibración: portar su código está bien, y diseñar las propias es libre;
  - especificaciones de formato: los hechos son libres.

## 2. Qué tiene, cómo funciona

- **Islas:**
  - un componente de una capa es isla si sus píxeles soportados por la capa anterior (brillo ≥ 150) son menos de `max(1, 0,25 × píxeles)`;
  - etiquetado por run-length;
  - en v7.1.1 se agregó un union-find entre capas.
- **Overhangs:** resta la capa anterior, umbraliza y erosiona 40 iteraciones.
- **Trampas de resina y copas de succión:** jerarquía de contornos (los agujeros son contornos negativos) más un **air map en dos pasadas**.
  - **De abajo hacia arriba:** un hueco que no se conecta con aire es candidato a trampa; si se superpone con aire en ≥ 10 px, drena.
  - **De arriba hacia abajo:** un candidato que se conecta con aire por arriba es una **copa de succión** (área ≥ 100 px y altura ≥ 0,5 mm).
- **Por capa:**
  - área = píxeles × área de píxel; volumen = área × altura de capa;
  - **no calcula perímetro**;
  - la estimación de tiempo **ignora la aceleración**.
- **Calculadoras:** light-off delay (incluye TSMC), mm↔px e inclinación óptima `atan(capa/XYres)`.
- **Operaciones:** Dynamic Lifts (altura y velocidad interpoladas linealmente según el área de la capa anterior), wait-before-cure proporcional al área, Pixel Dimming, Light Bleed Compensation, Raft Relief (detecta soportes con HoughCircles), Infill, Morph, Arithmetic, etc.
- **Calibración:**
  - Exposure Finder (por tiempo, brillo o altura de capa);
  - Elephant Foot (erosión y dimming);
  - XYZ Accuracy (`escala = real × 100 / observado`, con redondeo a píxel);
  - Tolerance, Blooming, Grayscale;
  - Lift Height (medida a mano con dinamómetro);
  - Stress Tower.
- **Formatos:** unos 40 lectores y escritores, más códecs RLE de referencia.

## 3. Peel y resistencia: qué calcula de verdad

- **No estima fuerza física.** La "peel curve" es **área / área máxima**.
- Usa umbrales fijos:
  - spike de peel: el área sube ≥ 35 % y más de 4 mm²;
  - fast peel: más del 40 % del área máxima con velocidad > 90 (unidad ambigua);
  - short lift: más del 45 % con altura < 2,5 mm.
- No usa perímetro, ni vacío de copas, ni reparto de la fuerza entre los soportes.

**Cómo superarlo** (alimenta BS-11 / #11):

- **Fuerza por capa:** `F_i = σ_adh·A_i + k_p·P_i + ΔP_vac·A_copa,i + F_visc(v, h)`, con área, **perímetro** (el frente de peel avanza desde el borde), copas activas y un término viscoso tipo Stefan.
- **Coeficientes:** se calibran con nuestras probetas (#79) más una prueba con dinamómetro.
- **Reparto:** por componente y por área tributaria de cada punta, acumulado hacia la base. Con eso se chequean tracción, flexión y pandeo con el módulo en verde.
- **Islas con margen** en vez de "soportado sí o no".
- **Ventaja nuestra:** tenemos la malla y el árbol de soportes. UVtools solo ve píxeles después del slice y no distingue modelo de soporte.

## 4. Ranking de reuso

| # | Qué | Acción | Área | Esfuerzo |
|---|---|---|---|---|
| 1 | Air map de 2 pasadas (trampas y copas) | Portar o reimplementar | Detección de problemas y fuerza | M |
| 2 | Islas por fracción soportada + union-find entre capas | Reimplementar | Detección de problemas | S–M |
| 3 | Modelo de fuerza por capa calibrable | Diseño propio | Análisis de soportes (#11) | L |
| 4 | Generadores de calibración | Portar (AGPL OK) o rediseñar | Calibración (#78, #79) | M c/u |
| 5 | Calculadoras | Reimplementar | UX | XS |
| 6 | Estimación de tiempo con aceleración y TSMC | Hacerla mejor | Slice / UX | S |
| 7 | Dynamic lifts y wait guiados por la fuerza estimada | Mejorar | Análisis de soportes | S (después del 3) |
| 8 | CTB/CBDDLP/PHZ | Desde catibo (BSD-2) | Formatos | M c/u |
| 9 | Dimming, light bleed, morph, infill, raft relief | Reimplementar si hay demanda | Post-proceso | S c/u |
| 10 | CTB cifrado | **No** sin revisión legal | Formatos | — |

## Fuentes

- https://github.com/sn4k3/UVtools: `LICENSE`, `CREDITS.md`, `UVtools.Core/Managers/IssueManager.cs`, `Layers/LayerIssueConfiguration.cs`, `Layers/Layer.cs`, `Operations/OperationCalculator.cs`, `OperationDynamicLifts.cs`, `OperationCalibrate*.cs`, `Suggestions/SuggestionWaitTimeBeforeCure.cs`, `FileFormats/CTBEncryptedFile.cs`, `UVtools.UI/Controls/LayerAreaPeelCurveControl.cs`.
- catibo (Rust, BSD-2): https://github.com/cbiffle/catibo
- uv3dp (Go, MIT): https://github.com/ezrec/uv3dp
- Licencia de Emgu CV: https://www.emgu.com/wiki/index.php/Licensing:
