# Arquitectura y modelo de seguridad

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## Límites del producto

`Laboratorio Libvirt` administra infraestructura efímera. No conoce qué
aplicación se instala, qué pruebas se ejecutan ni cómo se interpreta el
resultado. Los consumidores solo expresan una plantilla registrada, un
identificador de ejecución y una transición permitida.

```text
                      máquina Linux confiable

 CLI ─────────┐
              ├──► entrada tipada ─► casos de uso ─► puertos
 API HTTP v1 ─┘                            │             │
                                          ▼             ├── catálogo JSON
                                     dominio puro       ├── libvirt
                                                        ├── recibos JSON
                                                        ├── resultados
                                                        └── reloj

 consumidor ─► conector Windows/Linux ─► máquina virtual efímera
```

Los módulos se organizan así:

- `dominio`: identificadores, plantillas, políticas de red, reservas y máquina
  de estados;
- `aplicacion`: puertos, órdenes y casos de uso;
- `adaptadores`: configuración JSON, CLI, API, libvirt, recibos y resultados;
- `presentacion`: catálogos de mensajes e internacionalización;
- `composicion`: ensamblado explícito de adaptadores y casos de uso.

No se permite importar un adaptador desde el dominio o desde otro adaptador
salvo tipos privados de composición claramente delimitados. Las referencias
de libvirt nunca forman parte de una respuesta pública.

## Plantillas

Una plantilla pública contiene únicamente:

- identificador opaco;
- familia `windows` o `linux`;
- política `aislada` o `sin_red`;
- capacidades declarativas;
- canal de acceso opcional sin credenciales.

El inventario privado asocia ese identificador con el dominio, UUID, disco,
pool, volumen y red reales. Antes de crear una reserva se comprueba:

1. dominio apagado;
2. ausencia de estado de memoria administrado;
3. UUID exacto;
4. un único disco de sistema;
5. formato `qcow2`;
6. origen coincidente con el volumen registrado;
7. red activa y sin `<forward>`, cuando la política es aislada.

Windows y Linux son metadatos. El caso de uso no contiene ramas por sistema
operativo. UEFI, BIOS y TPM opcional se resuelven en el adaptador libvirt.

## Derivación de instancias

El adaptador crea un volumen incremental nuevo con el volumen registrado como
respaldo. Después deriva el XML inactivo de la plantilla y:

- sustituye el nombre y elimina el UUID;
- vacía el estado NVRAM para que libvirt cree uno independiente;
- elimina rutas de estado TPM copiadas;
- elimina MAC para que libvirt asigne otra;
- conserva solo el disco de sistema y apunta al volumen incremental;
- retira CD-ROM, `filesystem`, `hostdev`, `redirdev` y el canal SPICE del
  agente;
- elimina fuentes de canales ligadas al anfitrión;
- desactiva portapapeles y transferencia de ficheros SPICE;
- conecta exactamente una interfaz a la red registrada o elimina todas las
  interfaces para `sin_red`;
- elimina etiquetas y extensiones de línea de órdenes heredadas.

Tras definir el clon vuelve a leer el XML y verifica volumen, red, dispositivos
compartidos e independencia de UUID, NVRAM, TPM y MAC. Si cualquier comprobación
falla, retira la definición y el volumen exactos antes de devolver el error.

## Máquina de estados

```text
preparada ─► en_ejecucion ─► detenida ─► resultados_protegidos ─► descartada
    ▲              │             │
    └──────────────┘             └────────► fallida ── doble aceptación ─► descartada
```

Una reserva detenida puede iniciarse de nuevo para admitir reinicios y pruebas
por fases. El descarte normal exige resultados ya copiados y verificados fuera
de la VM. El descarte fallido exige un motivo tipado y aceptación adicional de
la pérdida.

Cada cambio incrementa una revisión. El almacenamiento toma un bloqueo
exclusivo y compara la revisión anterior antes del reemplazo atómico, por lo
que una CLI y la API no pueden sobrescribirse silenciosamente.

Antes de preparar otra instancia se cuentan las reservas no descartadas. La
configuración admite entre 1 y 128 y utiliza 4 por defecto, evitando que un
consumidor agote sin límite los recursos del anfitrión.

## Recuperación tras interrupciones

El recibo se crea antes que los recursos. Si la preparación falla, se conserva
como `fallida` para mantener trazabilidad. Los descartes son idempotentes sobre
recursos ausentes.

La orden `reconciliar` repara solo dos ventanas inequívocas:

- recibo `preparada` e instancia encendida: persiste `en_ejecucion`;
- recibo `en_ejecucion` e instancia apagada: persiste `detenida`.

Cualquier otra divergencia se rechaza para que un operador la diagnostique. No
se fuerza el apagado ni se destruye una instancia encendida.

## API y autenticación

La API integrada escucha exclusivamente en loopback. Su token:

- se lee de un fichero ordinario, no de argumentos ni del repositorio;
- debe pertenecer al usuario del servicio y tener permisos `0600`;
- se compara mediante el SHA-256 y una comparación de tiempo constante;
- nunca se incluye en recibos, errores o registros.

Todas las rutas requieren autenticación. Las mutaciones repiten además el
identificador de ejecución. Los errores externos usan códigos estables y no
exponen rutas ni salidas de libvirt. La publicación remota debe hacerse mediante
un frontal TLS o mTLS.

## Resultados

El consumidor copia primero sus resultados a una raíz Linux privada y crea
`manifiesto-laboratorio.json`. El verificador exige:

- versión e identificador exactos;
- rutas relativas formadas solo por componentes normales;
- ficheros ordinarios sin enlaces simbólicos ni físicos;
- tamaño y SHA-256 exactos;
- límites de artefactos y bytes;
- ausencia de ficheros o directorios no declarados.

Solo entonces la reserva pasa a `resultados_protegidos`.

## Límites de confianza

Son confiables el anfitrión, la plantilla apagada, el inventario privado y los
resultados ya verificados en Linux. Son no confiables el huésped, la carga de
trabajo y cualquier manifiesto antes de su verificación.

El modelo protege frente a efectos normales del software invitado. No cubre
fallos del núcleo del anfitrión, de QEMU/libvirt o del hardware, ni habilita por
sí solo el análisis seguro de código malicioso avanzado.
