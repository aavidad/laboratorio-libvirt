# Instrucciones para agentes

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## Idioma

- Escribir documentación, ayuda, mensajes, identificadores propios y pruebas
  en castellano.
- Mantener el castellano como catálogo canónico y ampliar idiomas mediante el
  módulo de presentación, nunca con ramas en el dominio.
- Se exceptúan nombres impuestos por Rust, HTTP, OpenAPI, libvirt y formatos
  externos.

## Arquitectura

- Mantener arquitectura hexagonal estricta.
- El dominio no puede importar adaptadores ni APIs externas.
- CLI y API son adaptadores de entrada sobre los mismos casos de uso.
- Libvirt, JSON, ficheros, reloj e internacionalización son adaptadores o
  puertos sustituibles.
- Windows y Linux son capacidades declarativas; no introducir órdenes del
  huésped en el núcleo.
- El laboratorio no debe depender de `windows-analyzer` ni de un orquestador.

## Seguridad

- No guardar credenciales, tokens, direcciones privadas, imágenes, volcados ni
  configuraciones locales en Git o evidencias.
- No aceptar XML, rutas, nombres de recursos o órdenes arbitrarias desde la CLI
  o API.
- Ejecutar `/usr/bin/virsh` con argumentos directos, nunca mediante shell.
- No forzar apagados ni retirar instancias encendidas.
- Verificar nombre, volumen, red y dispositivos antes de retirar recursos.
- Mantener la API en loopback; la exposición remota corresponde a un frontal
  TLS o mTLS.
- Conservar recibos de fallos y exigir confirmación exacta en mutaciones.

## Calidad

Antes de considerar terminado un cambio ejecutar:

```bash
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

Las pruebas de integración reales deben usar plantillas registradas, nuevos
identificadores y limpieza exacta. No reutilizar resultados de campañas
anteriores como demostración.
