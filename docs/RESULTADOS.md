# Contrato de resultados

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

Cada ejecución posee un directorio privado con el mismo identificador. El
consumidor copia allí los artefactos antes de solicitar
`proteger-resultados` y añade `manifiesto-laboratorio.json`:

```json
{
  "version": 1,
  "id_ejecucion": "prueba-20260802-001",
  "artefactos": [
    {
      "ruta": "informes/resumen.json",
      "tamano_bytes": 1234,
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
    }
  ]
}
```

La ruta se interpreta siempre respecto al directorio de la ejecución. No se
admiten rutas absolutas, `.` o `..`, enlaces, entradas especiales, duplicados,
directorios adicionales ni ficheros sin declarar.

Las cuotas predeterminadas son 5.000 artefactos, 512 MiB de contenido y 2 MiB
para el manifiesto. Pueden reducirse o ampliarse de forma acotada mediante la
configuración local. El SHA-256 del propio manifiesto, el número de artefactos
y los bytes verificados se incorporan al recibo. El manifiesto no se incluye
como artefacto de sí mismo.

El laboratorio verifica resultados que ya están en el anfitrión; no define el
transporte desde la VM. Un conector debe copiar, sincronizar, cerrar los
ficheros y calcular hashes antes de publicar el manifiesto.
