# Instalación del servicio

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## Requisitos

- Linux con QEMU y libvirt;
- `/usr/bin/virsh` como fichero ordinario;
- `/usr/bin/qemu-img` como fichero ordinario si se habilita promoción;
- Rust estable para compilar;
- una cuenta de servicio sin inicio de sesión y acceso al grupo de libvirt;
- plantillas apagadas y pools administrados previamente.

Para publicar accesos SSH, la plantilla debe incluir QEMU Guest Agent y crear
una marca UUID y una clave pública Ed25519 en las rutas fijas del perfil
configurado. El laboratorio lee ambos ficheros por el canal del hipervisor,
valida el dominio y el UUID y deriva la huella; no consulta la clave por la red
SSH. El procedimiento y las ACL requeridas se describen en
[IDENTIDAD_SSH.md](IDENTIDAD_SSH.md).

## Binario

```bash
cargo build --release
```

Instale `target/release/laboratorio-libvirt` como
`/usr/bin/laboratorio-libvirt`. El repositorio no ejecuta automáticamente
operaciones con privilegios.

## Cuenta y directorios

El administrador puede crear una cuenta denominada `laboratorio-libvirt`,
añadirla al grupo que autoriza la conexión local a libvirt y aplicar
`distribucion/systemd/laboratorio-libvirt.tmpfiles` mediante el mecanismo de
`systemd-tmpfiles` de la distribución.

Las raíces deben quedar con modo `0700`. La configuración se instala como
`/etc/laboratorio-libvirt/configuracion.json`, propiedad de la cuenta de
servicio y modo `0600`.

La versión 0.2 exige el formato de configuración 2. Una configuración 1 se
rechaza para evitar mantener accesos SSH sin identidad fuera de banda.

Genere el token mediante una fuente criptográfica del sistema y guárdelo en la
ruta configurada con modo `0600`. No lo coloque en la unidad systemd, variables
de entorno, argumentos ni registros.

## Unidad systemd

La unidad de referencia está en
`distribucion/systemd/laboratorio-libvirt.service`. Mantiene la API en
loopback, aplica una máscara privada, restringe el sistema de ficheros y deja
escribible únicamente la raíz del producto. Revise el nombre del servicio de
libvirt y las rutas de la distribución antes de instalarla.

Después de instalar y recargar systemd:

```text
systemctl enable --now laboratorio-libvirt.service
systemctl status laboratorio-libvirt.service
```

No copie la salida del entorno, el token ni direcciones de máquinas a informes
públicos. Para exposición remota configure un frontal TLS o mTLS independiente
y mantenga el proceso enlazado a loopback.
