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
| `GET` | `/api/v1/destinos-promocion` | destinos públicos preautorizados |
| `GET` | `/api/v1/plantillas/{id}/diagnostico` | invariantes observadas |
| `GET` | `/api/v1/reservas` | recibos ordenados por identificador |
| `GET` | `/api/v1/reservas/{id}` | recibo persistido |
| `GET` | `/api/v1/reservas/{id}/acceso` | canal e identidad pública autenticada fuera de banda |
| `GET` | `/api/v1/reservas/{id}/diagnostico-acceso` | comprobaciones saneadas de disponibilidad |
| `GET` | `/api/v1/reservas/{id}/diagnostico-arranque` | precondiciones y último fallo saneado de arranque |

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

## Diagnóstico saneado de acceso

`GET /api/v1/reservas/{id}/diagnostico-acceso` no devuelve el punto de acceso.
Publica `preparado` y comprobaciones formadas exclusivamente por `codigo`,
`aplicable`, `bloqueante` y `correcta`. `direccion_observable` es la condición
agregada bloqueante; `lease`, agente y ARP son fuentes alternativas
informativas, por lo que una fuente ausente no contradice un diagnóstico
preparado si otra sí observa la dirección. Los demás códigos cubren reserva e
instancia, QEMU Guest Agent y presencia y validez de la marca UUID y la clave
pública SSH.

Una respuesta de diagnóstico nunca contiene IP, MAC, nombres de dominio o red,
rutas, XML ni salida de `virsh`. Los errores esperados al observar una fuente se
reducen a `correcta: false`.

## Diagnóstico saneado de arranque

`GET /api/v1/reservas/{id}/diagnostico-arranque` es una consulta de solo
lectura. Comprueba mediante códigos y booleanos que la reserva sea iniciable,
la instancia esté registrada y apagada, su definición y almacenamiento sean
válidos, ningún dominio activo mantenga para escritura el disco o un backing
que necesita la reserva, las redes requeridas estén activas y no haya estado guardado. Si el
hipervisor conserva un fallo, `ultimo_fallo` solo admite un catálogo cerrado,
como `bloqueo_escritura_recurso`; no publica registros, dominios, rutas, UUID,
IP ni argumentos internos. Un fallo histórico puede coexistir con
`iniciable: true` cuando sus precondiciones actuales ya vuelven a ser válidas.
La orden `iniciar` exige este mismo diagnóstico justo antes de solicitar el
arranque; si `recursos_escritura_disponibles` es falso, no intenta arrancar ni
modifica el recibo.

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

Los identificadores tienen de 3 a 64 bytes ASCII, comienzan por minúscula o
dígito y solo contienen minúsculas, dígitos o guion. El servidor no transforma
mayúsculas, guiones bajos ni ninguna otra variante: los rechaza.

## Preparación y promoción de plantilla

Una reserva debe haberse iniciado para aprovisionarla y estar detenida antes de
sanearla. El saneamiento elige únicamente un destino opaco registrado:

`POST /api/v1/reservas/{id}/preparacion/sanear`

```json
{
  "id_destino": "windows-analisis-siguiente",
  "confirmacion": "prueba-20260802-001"
}
```

Después se invocan, con el cuerpo normal de confirmación, las tres primeras
rutas dos veces y finalmente la cuarta:

| Método | Ruta |
|---|---|
| `POST` | `/api/v1/reservas/{id}/preparacion/iniciar-ciclo` |
| `POST` | `/api/v1/reservas/{id}/preparacion/detener-ciclo` |
| `POST` | `/api/v1/reservas/{id}/preparacion/validar-ciclo` |
| `POST` | `/api/v1/reservas/{id}/preparacion/promover` |

El servidor observa las invariantes; el cliente no envía resultados de
validación, nombres libvirt, XML, redes, rutas o volúmenes. El recibo registra
las dos comprobaciones y termina en `promovida`. Si el destino usa SSH, cada
ciclo conserva además una comprobación `identidad_ssh` obtenida por QGA antes
del apagado, incluida la huella SHA-256 canónica; no se acepta el ciclo si la
marca UUID o la clave de host fallan, ni se promueve si la huella cambia entre
los dos ciclos. La plantilla anterior se conserva.

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
    "motivo_fallo": null,
    "preparacion_plantilla": null
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

Un punto de acceso SSH contiene `identidad_servidor` con algoritmo, clave
pública y huella SHA-256. El dato procede de la clave pública OpenSSH leída por
QEMU Guest Agent y queda ligado a la instancia por el dominio exacto y una
marca coincidente con su UUID XML. Si no existe una identidad válida, la
consulta se rechaza; el cliente no debe aplicar TOFU ni `ssh-keyscan`.
