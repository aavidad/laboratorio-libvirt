# Integración con analizadores y orquestadores

Copyright (C) 2026 Alberto Avidad

SPDX-License-Identifier: GPL-3.0-or-later

## Principio

El laboratorio entrega infraestructura; el consumidor entrega el trabajo. No
hay una dependencia de código entre ambos. Una integración usa la CLI o la API,
selecciona una capacidad y conserva el identificador opaco de la reserva.

```text
orquestador / analizador
          │
          ├── solicita plantilla por capacidad
          ├── prepara e inicia una reserva
          ├── consulta el punto de acceso
          └── ejecuta su conector de huésped
                         │
                         ▼
                VM efímera Windows/Linux
```

## Integración con `windows-analyzer`

La opción recomendada utiliza dos niveles de imagen:

1. `windows-limpio`: sistema controlado sin aplicaciones objetivo;
2. `windows-analisis-<version>`: derivada reproducible que incorpora una
   versión concreta y verificada de `windows-analyzer`, Fibratus y el bootstrap
   de ejecución interactiva.

La plantilla instrumentada no contiene la aplicación que se analizará. Su
capacidad pública puede llamarse `analisis-windows`; el nombre y la versión
concretos siguen siendo responsabilidad del catálogo del consumidor.

Flujo de una campaña:

1. seleccionar una plantilla con `analisis-windows`;
2. inspeccionarla y exigir todas las comprobaciones correctas;
3. preparar e iniciar una reserva nueva;
4. obtener el canal de acceso registrado;
5. comprobar dentro del huésped el SHA-256 y la versión del analizador;
6. iniciar la observación antes de ejecutar el instalador objetivo;
7. ejecutar instalación, reinicios y aplicación mediante solicitudes tipadas;
8. recorrer semánticamente menús y controles en la sesión gráfica;
9. copiar los resultados al anfitrión y crear el manifiesto;
10. detener, proteger resultados y descartar la reserva.

El conector Windows mantiene una sesión SSH persistente para coordinación y
usa tareas `InteractiveToken` para la interfaz. No pasa contraseñas por la API
del laboratorio ni permite PowerShell arbitrario.

## Integración con un orquestador general

Un orquestador puede registrar plantillas como:

- `windows-compilacion`;
- `windows-pruebas-escritorio`;
- `linux-pruebas-rust`;
- `linux-pruebas-web-sin-red`.

El laboratorio publica capacidades, sistema, política de red y canal. El
orquestador decide qué conector autorizado usar y qué artefactos recuperar. Las
credenciales pertenecen al conector o a un almacén de secretos externo, nunca
al recibo de la reserva.

## Idempotencia del consumidor

El consumidor debe conservar `id_ejecucion` y tratar cada transición como una
operación confirmada. Tras perder una respuesta:

1. consultar `estado`;
2. si el estado observado de libvirt puede haber avanzado, ejecutar
   `reconciliar`;
3. repetir únicamente la transición que siga siendo válida;
4. no generar otro identificador para ocultar una reserva anterior.

Los resultados se consideran recuperados solo después de
`proteger-resultados`. Haber terminado la prueba dentro de la VM no autoriza el
descarte.

## Lo que no debe acoplarse

- El laboratorio no debe importar el motor del analizador.
- El analizador no debe enlazar libvirt ni destruir la VM que lo ejecuta.
- El orquestador no debe enviar XML, nombres de dominios, rutas o órdenes del
  anfitrión.
- Un conector de auditoría futuro debe consumir resultados saneados, no formar
  parte del dominio de reservas.
