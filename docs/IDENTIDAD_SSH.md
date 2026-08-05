# Identidad SSH fuera de banda

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## Objetivo

El punto de acceso de una reserva SSH no se considera autenticable solo por
conocer su dirección. El laboratorio lee mediante QEMU Guest Agent la clave
pública OpenSSH del huésped y una marca fija con su UUID. Después las contrasta
con el dominio exacto ligado al recibo de reserva y entrega la huella al
conector.

No se admite una clave enviada por CLI o API, no se ejecuta `ssh-keyscan` y no
se acepta la primera clave vista por la red.

## Perfiles y rutas fijas

El inventario privado selecciona uno de estos perfiles:

| Perfil | Marca UUID | Clave pública Ed25519 |
|---|---|---|
| `windows_openssh` | `C:\ProgramData\LaboratorioAplicaciones\estado\uuid-identidad-ssh.txt` | `C:\ProgramData\ssh\ssh_host_ed25519_key.pub` |
| `linux_openssh` | `/var/lib/laboratorio-libvirt/identidad-ssh.uuid` | `/etc/ssh/ssh_host_ed25519_key.pub` |

La ruta no se recibe del consumidor. El adaptador usa únicamente las órdenes
QEMU Guest Agent `guest-file-open`, `guest-file-read` y `guest-file-close`; no
usa `guest-exec`. La marca admite como máximo 128 bytes y la clave pública 512
bytes.

## Contrato y enlace con la reserva

El huésped no conoce ni recibe `id_ejecucion` o `id_instancia`. El laboratorio
resuelve ambos desde su recibo persistido y dirige todas las consultas QGA al
dominio libvirt exacto. Antes de publicar el acceso exige:

1. que el nombre del XML del dominio coincida con `id_instancia`;
2. que el XML contenga un UUID canónico;
3. que la marca del huésped sea una única línea UUID canónica en minúsculas y
   coincida exactamente con el UUID canónico del XML;
4. que la clave sea una única línea canónica formada por `ssh-ed25519`, su
   blob Base64 y, opcionalmente, un comentario ordinario de un solo token y
   hasta 128 bytes; el comentario se valida pero no se publica;
5. que el blob use la estructura binaria OpenSSH real: cadena
   `ssh-ed25519`, cadena de clave de 32 bytes y ningún byte adicional.

La huella devuelta es `SHA256:` seguido del SHA-256 Base64 sin relleno del blob
OpenSSH completo. El perfil fija Ed25519 deliberadamente: no se negocian
algoritmos ni rutas a través de CLI o API.

## Publicación por el bootstrap

El bootstrap autorizado obtiene el UUID desde el propio sistema. Si la marca
falta o no coincide, primero la invalida, detiene SSH, regenera únicamente las
claves de host inventariadas y confirma que `sshd` usa la clave Ed25519. Debe
validar la configuración, iniciar el servicio y escribir la marca de forma
atómica **al final**. Ante cualquier fallo la marca queda ausente y el
laboratorio falla de forma cerrada.

En Windows, la marca vive en la raíz privada ya inventariada de
`LaboratorioAplicaciones\estado` y conserva su ACL administrada por el bootstrap.
En Linux, la marca pertenece a `root` y usa modo `0600`. La clave privada
conserva las ACL estrictas de OpenSSH y nunca se lee por QGA; la clave `.pub` es
material público, pero debe ser un fichero ordinario en la ruta fija y
corresponder a la clave efectiva de `sshd`.

## Uso por el conector

El conector obtiene `PuntoAcceso`, crea un almacén temporal de `known_hosts`
con la clave exacta recibida y exige comprobación estricta. Debe descartar el
almacén con la reserva. Si la clave ofrecida por SSH difiere, la campaña se
detiene como fallo de identidad; nunca se actualiza automáticamente.

El recibo del laboratorio liga ejecución e instancia; el dominio exacto y la
marca ligan esa instancia con su UUID, sin inyectar identificadores dinámicos
en el huésped ni crear una dependencia circular. QEMU Guest Agent aporta un
canal fuera de la red SSH, pero no convierte un huésped administrador ya
comprometido en una raíz de confianza. Esta autenticación evita un intermediario
de red y detecta rotaciones entre la lectura y la conexión; no acredita la
integridad general del sistema invitado.

La aceptación de una nueva plantilla repite esta validación en cada uno de sus
dos ciclos mientras la candidata sigue activa. El recibo persiste una
`ComprobacionIdentidadSsh` antes del apagado y la validación del ciclo la
incorpora definitivamente con la huella `SHA256:` canónica. Las huellas de los
dos ciclos deben ser idénticas; una regeneración de clave entre reinicios
detiene la aceptación y no se promueve el destino.
