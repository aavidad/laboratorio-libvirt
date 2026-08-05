# Laboratorio Libvirt

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

`Laboratorio Libvirt` es una fábrica independiente de máquinas virtuales
efímeras Windows y Linux. Crea cada instancia como un volumen `qcow2`
incremental sobre una plantilla apagada, aplica una política de aislamiento,
persiste un recibo recuperable y retira exclusivamente los recursos que creó.

El producto se controla mediante una CLI en castellano o una API HTTP v1
tipada. Un analizador, un sistema de integración continua o un orquestador
pueden consumir ambos contratos sin enlazar libvirt ni conocer nombres de
dominios, pools, volúmenes o redes.

## Responsabilidades

El laboratorio:

- registra plantillas Windows y Linux mediante identificadores opacos;
- verifica que la plantilla está apagada, sin estado guardado y con identidad,
  disco y red conformes al inventario privado;
- crea un volumen incremental nuevo y una definición libvirt con UUID, MAC,
  NVRAM y estado TPM independientes cuando correspondan;
- elimina CD-ROM, carpetas compartidas, dispositivos del anfitrión, redirección
  USB, portapapeles y transferencia de ficheros SPICE;
- admite plantillas conectadas a una red aislada sin reenvío o completamente
  desconectadas;
- arranca, solicita apagado cooperativo y reconcilia interrupciones inequívocas;
- publica un punto de acceso sin credenciales cuando la plantilla lo declara;
- liga los accesos SSH a una clave de host leída fuera de banda por el canal
  autenticado del hipervisor, sin TOFU ni `ssh-keyscan`;
- sanea medios y red desde inventario privado, exige dos ciclos de arranque,
  apagado y validación, y promueve una plantilla nueva sin reemplazar la vieja;
- protege resultados mediante manifiesto, cuotas y SHA-256 antes del descarte;
- conserva recibos atómicos con control de revisión y bloqueo entre procesos.

No instala sistemas operativos, no ejecuta órdenes arbitrarias dentro del
huésped y no interpreta las pruebas. Esas funciones pertenecen a conectores
independientes. Consulte [la integración con analizadores y
orquestadores](docs/INTEGRACION.md).

## Arquitectura

El proyecto sigue arquitectura hexagonal:

```text
            CLI              API HTTP v1
             │                    │
             └─────────┬──────────┘
                       ▼
                casos de uso
                       │
      ┌────────────────┼─────────────────┐
      ▼                ▼                 ▼
 catálogo         proveedor VM      recibos/resultados
      │                │                 │
 JSON privado      libvirt CLI       ficheros privados
                       │
                  dominio puro
          plantillas, reservas y estados
```

El dominio no importa HTTP, JSON, libvirt, procesos, red ni sistema de
ficheros. La raíz de composición es el único módulo que ensambla puertos y
adaptadores concretos. La descripción completa está en
[Arquitectura y seguridad](docs/ARQUITECTURA.md).
El contrato de clave de host se detalla en
[Identidad SSH fuera de banda](docs/IDENTIDAD_SSH.md).

## Compilación

Requiere Rust estable, libvirt/QEMU y `/usr/bin/virsh`. La promoción también
requiere `/usr/bin/qemu-img`:

```bash
cargo build --release
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

El binario resultante es `target/release/laboratorio-libvirt`.
Para una instalación como servicio consulte
[Instalación y unidad systemd](docs/INSTALACION.md).

## Configuración local

Copie [config.example.json](config.example.json) fuera del repositorio. La
configuración contiene referencias locales y debe ser un fichero ordinario
privado. Las raíces de recibos y resultados deben existir, pertenecer al
usuario del servicio y tener permisos `0700`. El token de la API debe tener al
menos 32 caracteres, ser un fichero ordinario del mismo usuario y tener
permisos `0600`.

Todos los identificadores públicos usan entre 3 y 64 bytes ASCII, empiezan por
minúscula o dígito y solo admiten minúsculas, dígitos y guion. Se comparan de
forma exacta y nunca se normalizan.

La API integrada rechaza cualquier dirección que no sea loopback. Para acceso
desde otra máquina debe colocarse detrás de un frontal TLS o mTLS que mantenga
el servicio enlazado localmente.

La versión 0.2 usa configuración `version: 2`. Al migrar desde 0.1, cada
plantilla con canal SSH debe declarar `perfil_identidad_acceso`; no se conserva
un modo TOFU de compatibilidad. Los destinos de promoción se añaden en
`promociones` y pueden quedar vacíos si esa capacidad no se utiliza.

## CLI

La ayuda canónica está en castellano:

```bash
laboratorio-libvirt --ayuda
```

Consultas:

```bash
laboratorio-libvirt --configuracion <configuracion.local.json> plantillas
laboratorio-libvirt --configuracion <configuracion.local.json> reservas
laboratorio-libvirt --configuracion <configuracion.local.json> destinos-promocion
laboratorio-libvirt --configuracion <configuracion.local.json> \
  inspeccionar <id-plantilla>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  estado <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  acceso <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  diagnosticar-acceso <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  diagnosticar-arranque <id-ejecucion>
```

`diagnosticar-acceso` es una consulta saneada: devuelve únicamente códigos y
booleanos para estado de la instancia, fuentes de dirección, QEMU Guest Agent
y presencia y validez de la marca UUID y la clave pública SSH. No publica IP,
MAC, nombres, rutas ni salida de libvirt. `direccion_observable` es bloqueante;
las fuentes `lease`, agente y ARP son alternativas informativas identificadas
como no bloqueantes.

`diagnosticar-arranque` comprueba de forma no mutante que el recibo sea
iniciable, que dominio, almacenamiento y redes estén disponibles y que no haya
un estado guardado. Cuando libvirt conserva un fallo de arranque, lo reduce a
un código cerrado; nunca devuelve nombres, rutas, UUID, direcciones ni el texto
del registro. En la campaña del 5 de agosto de 2026 permitió identificar un
`bloqueo_escritura_recurso`. La comprobación
`recursos_escritura_disponibles` inspecciona también la cadena de backing y
evita `iniciar` mientras otro dominio activo mantenga el recurso para
escritura, sin publicar cuál es.

Las mutaciones exigen repetir el identificador exacto:

```bash
laboratorio-libvirt --configuracion <configuracion.local.json> \
  preparar <id-plantilla> <id-ejecucion> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  iniciar <id-ejecucion> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  detener <id-ejecucion> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  reconciliar <id-ejecucion> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  proteger-resultados <id-ejecucion> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  descartar <id-ejecucion> --confirmar <id-ejecucion>
```

Una ejecución sin resultados solo puede retirarse tras marcarla como fallida y
aceptar expresamente la pérdida. No existe apagado forzado.

La preparación de una plantilla utiliza una reserva detenida y un destino
preautorizado en el inventario privado:

```bash
laboratorio-libvirt --configuracion <configuracion.local.json> \
  sanear-plantilla <id-ejecucion> <id-destino> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  iniciar-ciclo-plantilla <id-ejecucion> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  detener-ciclo-plantilla <id-ejecucion> --confirmar <id-ejecucion>
laboratorio-libvirt --configuracion <configuracion.local.json> \
  validar-ciclo-plantilla <id-ejecucion> --confirmar <id-ejecucion>
# Repetir iniciar/detener/validar una segunda vez.
laboratorio-libvirt --configuracion <configuracion.local.json> \
  promover-plantilla <id-ejecucion> --confirmar <id-ejecucion>
```

El saneamiento solo retira los medios exactos y restaura la red exacta que
figuran en la configuración privada. La promoción aplana el disco en un
volumen y dominio nuevos; rechaza cualquier destino que ya sea una plantilla
registrada y conserva intacta la plantilla de origen. En destinos SSH, cada
ciclo valida por QEMU Guest Agent la marca UUID y la clave de host mientras la
máquina sigue activa y persiste el resultado antes de aceptar el apagado.

## API

```bash
laboratorio-libvirt --configuracion <configuracion.local.json> servir-api
```

Todas las rutas, incluida salud, exigen `Authorization: Bearer …`. La API
limita las solicitudes a 64 KiB, valida identificadores durante la
deserialización y redacta los errores de infraestructura. El contrato se
documenta en [API HTTP v1](docs/API_V1.md) y en
[OpenAPI 3.1](docs/api-v1.yaml).

## Internacionalización

Los nombres del dominio, el código propio, la documentación y el catálogo
canónico están en castellano. La presentación usa un puerto de traducción y un
catálogo independiente en `recursos/i18n/es.json`. La configuración puede
registrar un `directorio_idiomas` con catálogos adicionales `<idioma>.json`;
estos completan el catálogo castellano sin modificar las reglas del dominio.
Si un idioma solicitado no está instalado o no es válido, se utiliza
castellano.

## Estado de seguridad

El laboratorio contiene los efectos normales de una carga de trabajo dentro
de un clon desechable. No afirma contener una vulnerabilidad del hipervisor ni
sustituir un laboratorio profesional de análisis de código malicioso. No se
deben incorporar secretos, direcciones privadas, imágenes, resultados ni
configuraciones locales al repositorio.
