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

## Identidad de acceso fuera de banda

La dirección observada por DHCP o Guest Agent no autentica un servidor SSH.
Cuando una plantilla declara SSH, su inventario debe seleccionar un perfil de
identidad fija. El adaptador lee por QEMU Guest Agent una marca UUID y la clave
pública Ed25519 efectiva en rutas determinadas por ese perfil, sin ejecutar
órdenes dentro del huésped.

El caso de uso resuelve la instancia desde el recibo persistido, exige que el
nombre del XML corresponda a ella, compara la marca con el UUID del XML, valida
la estructura binaria OpenSSH real y calcula la huella SHA-256 que incluye en
`PuntoAcceso`. Si falta o no coincide, no publica el acceso. Ni CLI ni API
aceptan claves de host, y el producto no usa TOFU, `ssh-keyscan` ni la propia
conexión SSH como raíz de confianza.

El caso de uso de diagnóstico de acceso usa otro puerto de solo lectura. El
adaptador consulta las fuentes fijas `lease`, `agent` y `arp`, comprueba
`guest-ping`, la marca UUID y la clave pública y reduce cada resultado a
booleanos. La respuesta solo contiene códigos estables; las direcciones, MAC,
nombres, rutas, XML y salidas del hipervisor permanecen dentro del adaptador.
Las tres fuentes de dirección son alternativas informativas y una comprobación
agregada `direccion_observable` es la única bloqueante para esa condición.

El diagnóstico de arranque usa un puerto independiente y no mutante. Observa
solo precondiciones fijas de recibo, dominio, almacenamiento, red y estado
guardado. La razón del último fallo se reduce dentro del adaptador a un enum
cerrado; nunca atraviesa el puerto el texto de libvirt ni identificadores o
rutas del anfitrión.
La comprobación de almacenamiento incluye la cadena de backing declarada por
el volumen. Se compara internamente con los recursos abiertos para escritura
por los dominios activos; solo se publica
`recursos_escritura_disponibles`. El caso de uso vuelve a exigirla justo antes
de arrancar para no repetir un fallo de bloqueo conocido.

## Preparación de una plantilla nueva

Los destinos de promoción viven en el inventario privado. Cada uno fija un
origen admitido, identidad pública, dominio y UUID de destino, pools, volumen,
disco, red temporal, red final y medios temporales exactos. Las fronteras
públicas solo admiten el identificador opaco del destino.
Los medios temporales no pueden repetir destino ni recurso, ocupar el destino
del disco sistema o presentarse con escritura habilitada: solo se retira un
medio inventariado y explícitamente de solo lectura.

Sobre una reserva ya aprovisionada y apagada se aplica esta máquina de estados:

```text
detenida
  └─ sanear ─► saneada
                  └─ arrancar/apagar ─► ciclo_1_en_curso ─► ciclo_1_validado
                                                               └─ arrancar/apagar
                                                                  ─► ciclo_2_en_curso
                                                                  ─► ciclo_2_validado
                                                                      └─ promover
                                                                         ─► promocion_en_curso
                                                                         ─► promovida
```

Cada validación vuelve a observar apagado, ausencia de medios, red final,
disco único e identidad independiente y queda incorporada al recibo. No se
acepta una atestación booleana enviada por el consumidor. El registro conserva
por ciclo los instantes de inicio, apagado y validación y las cinco
comprobaciones observadas. Para un destino SSH, antes de solicitar el apagado
el proveedor valida por QGA la marca UUID y la clave de host del dominio activo;
la comprobación tipada y su instante quedan persistidos y son obligatorios para
aceptar cada ciclo. Un destino sin SSH conserva la misma comprobación como no
aplicable. El recibo conserva además la huella SHA-256 de cada ciclo y la
promoción exige que ambas sean canónicas e idénticas, evitando aceptar una
imagen que regenere su clave de host en cada arranque.

El saneamiento redefine solo la candidata apagada y revierte el XML anterior
si la lectura posterior no cumple las invariantes. La promoción crea un volumen
nuevo, aplana la cadena incremental con `qemu-img convert`, redefine la
candidata con el nombre y UUID inventariados, valida el resultado y solo
entonces retira el clon y su volumen. Ante un fallo anterior a esa validación,
restaura la candidata y retira el destino incompleto. La plantilla de origen no
se redefine, no se escribe y no se sobrescribe.

Antes de crear recursos de destino, el caso de uso exige que estén ausentes y
persiste `promocion_en_curso`. Esta fase es el recibo de propiedad que permite
reanudar o retirar un volumen incompleto tras una interrupción. Sin ella, un
recurso preexistente se rechaza y nunca se elimina.

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

Además, una segunda guarda por reserva serializa desde la primera observación
hasta terminar el efecto de hipervisor y persistir el recibo. El CAS protege la
escritura y esta guarda evita efectos concurrentes, por ejemplo dos conversiones
del mismo volumen de promoción.

Antes de preparar otra instancia se cuentan las reservas no descartadas. La
configuración admite entre 1 y 128 y utiliza 4 por defecto, evitando que un
consumidor agote sin límite los recursos del anfitrión.

El identificador público se conserva sin normalización. Para mantener el
espacio privado `lab-` dentro del máximo común de 64 caracteres, el productor
usa `lab-<id>` cuando cabe y, para entradas más largas, `lab-` seguido de 240
bits de SHA-256 en hexadecimal. Así cualquier identificador público válido
produce un nombre interno válido sin truncamientos susceptibles de colisión.

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
