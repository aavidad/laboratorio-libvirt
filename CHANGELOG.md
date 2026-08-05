# Historial de cambios

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## 0.2.0 - 2026-08-04

- Flujo tipado para sanear candidatas, registrar dos ciclos de reinicio y
  validación y promover una plantilla nueva sin sobrescribir la anterior.
- Inventario privado de CD, medios temporales, redes y destinos de promoción;
  CLI y API solo aceptan identificadores opacos.
- Promoción transaccional con disco `qcow2` aplanado, rollback e idempotencia.
- Bloqueo de operación por reserva alrededor de efectos externos y recibos.
- Identidad SSH ligada al dominio exacto mediante una marca UUID y la clave
  pública OpenSSH leídas por QEMU Guest Agent; se elimina la dependencia de
  recibos dinámicos, TOFU o `ssh-keyscan`.
- Configuración local versión 2; los canales SSH deben declarar su perfil de
  identidad fuera de banda.
- Diagnóstico de acceso tipado y saneado por CLI/API, sin direcciones ni datos
  internos del hipervisor.
- El diagnóstico de acceso distingue condiciones bloqueantes de fuentes
  alternativas informativas y añade `direccion_observable` como resultado
  agregado coherente con `preparado`.
- Diagnóstico de arranque de solo lectura por CLI/API, con precondiciones
  booleanas y catálogo cerrado de fallos. La campaña real clasificó el fallo
  como `bloqueo_escritura_recurso`; ahora inspecciona la cadena de backing y
  bloquea `iniciar` antes de repetir el intento si otro dominio conserva el
  recurso para escritura.
- Cada ciclo de aceptación de un destino SSH valida por QGA la marca UUID y la
  clave de host antes del apagado, conserva su huella en el recibo y exige que
  permanezca estable en los dos ciclos.
- Contrato único de identificadores: 3 a 64 caracteres ASCII, minúsculas,
  dígitos y guion, sin normalización.
- Los medios temporales de promoción rechazan destinos o recursos duplicados,
  el destino del disco sistema y cualquier medio presente sin solo lectura.
- La clave pública OpenSSH admite el comentario ordinario opcional de `ssh-keygen`
  pero publica siempre la representación canónica sin comentario.
- Las pruebas OpenAPI usan `serde_yaml_ng` en lugar del paquete obsoleto
  `serde_yaml`.

## 0.1.0 - 2026-08-02

- Repositorio y aplicación independientes.
- Dominio hexagonal de plantillas y reservas Windows/Linux.
- Adaptador libvirt con volúmenes incrementales y saneamiento de dispositivos.
- Políticas de red aislada y sin red.
- CLI y API HTTP v1 tipadas.
- Ayuda en castellano e infraestructura ampliable de internacionalización.
- Autenticación por token privado y escucha local obligatoria.
- Recibos atómicos con bloqueo y control de revisión.
- Reconciliación segura tras interrupciones.
- Verificación estricta de resultados y descarte protegido.
