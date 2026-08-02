# Instalación del servicio

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## Requisitos

- Linux con QEMU y libvirt;
- `/usr/bin/virsh` como fichero ordinario;
- Rust estable para compilar;
- una cuenta de servicio sin inicio de sesión y acceso al grupo de libvirt;
- plantillas apagadas y pools administrados previamente.

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
