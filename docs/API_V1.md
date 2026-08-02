# API HTTP v1

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## Transporte

La API se inicia con:

```bash
laboratorio-libvirt --configuracion <configuracion.local.json> servir-api
```

El servidor solo acepta una dirección loopback configurada. Todas las rutas
requieren una cabecera Bearer cuyo valor procede del fichero privado indicado
en la configuración:

```text
Authorization: Bearer <token-del-almacen-seguro>
Accept-Language: es
Content-Type: application/json
```

El cuerpo máximo es 64 KiB. Los objetos rechazan campos desconocidos y todos
los identificadores se validan al deserializar.

## Rutas de consulta

| Método | Ruta | Resultado |
|---|---|---|
| `GET` | `/api/v1/salud` | versión y disponibilidad |
| `GET` | `/api/v1/plantillas` | catálogo público |
| `GET` | `/api/v1/plantillas/{id}/diagnostico` | invariantes observadas |
| `GET` | `/api/v1/reservas` | recibos ordenados por identificador |
| `GET` | `/api/v1/reservas/{id}` | recibo persistido |
| `GET` | `/api/v1/reservas/{id}/acceso` | protocolo, dirección y puerto sin credenciales |

## Preparación

`POST /api/v1/reservas`

```json
{
  "id_plantilla": "windows-analisis",
  "id_ejecucion": "prueba-20260802-001",
  "confirmacion": "prueba-20260802-001"
}
```

Devuelve `201 Created` y un recibo `preparada`.

## Transiciones confirmadas

Estas rutas reciben el mismo cuerpo:

```json
{
  "confirmacion": "prueba-20260802-001"
}
```

| Método | Ruta |
|---|---|
| `POST` | `/api/v1/reservas/{id}/iniciar` |
| `POST` | `/api/v1/reservas/{id}/detener` |
| `POST` | `/api/v1/reservas/{id}/reconciliar` |
| `POST` | `/api/v1/reservas/{id}/proteger-resultados` |
| `POST` | `/api/v1/reservas/{id}/descartar` |

La confirmación se compara con el identificador de la ruta dentro del caso de
uso, no solo en el adaptador HTTP.

## Fallo y descarte excepcional

`POST /api/v1/reservas/{id}/fallar`

```json
{
  "motivo": "carga_trabajo",
  "confirmacion": "prueba-20260802-001"
}
```

Los motivos admitidos son `infraestructura`, `carga_trabajo`, `cancelada` y
`resultados_irrecuperables`.

`POST /api/v1/reservas/{id}/descartar-fallida`

```json
{
  "acepta_perdida_resultados": true,
  "confirmacion": "prueba-20260802-001"
}
```

Esta ruta solo funciona sobre una reserva ya marcada como fallida y apagada.

## Respuestas

Las respuestas de operación usan un discriminante estable:

```json
{
  "tipo": "recibo",
  "datos": {
    "version": 1,
    "revision": 1,
    "id_ejecucion": "prueba-20260802-001",
    "id_plantilla": "windows-analisis",
    "id_instancia": "lab-prueba-20260802-001",
    "sistema": "windows",
    "estado": "en_ejecucion",
    "creado_en_unix_ms": 0,
    "actualizado_en_unix_ms": 0,
    "resultados": null,
    "motivo_fallo": null
  }
}
```

Los errores externos no incluyen la causa de libvirt:

```json
{
  "codigo": "operacion_rechazada",
  "mensaje": "no se pudo completar la operación"
}
```

Estados principales: `400` para una solicitud no válida, `401` para
autenticación incorrecta, `409` para una transición o precondición rechazada y
`500` para un fallo de la tarea interna.

El contrato legible por herramientas está en [api-v1.yaml](api-v1.yaml).
